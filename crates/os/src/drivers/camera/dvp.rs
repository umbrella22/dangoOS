//! LCD_CAM + AXI DMA2 adapter. No sensor protocol or UI policy lives here.
use crate::drivers::memory::PixelMemory;
use alloc::boxed::Box;
use embassy_time::{Duration, with_timeout};
use esp_hal::{
    Async,
    dma::{
        DmaDescriptor, DmaRxBuf,
        aligned::{DmaAlignedMut, InternalMemory},
    },
    gpio::{Flex, Input, InputConfig, interconnect::InputSignal},
    lcd_cam::{
        LcdCam,
        cam::{Camera, Config, EofMode},
    },
    peripherals::{DMA_AXI_CH2, GPIO15, GPIO16, GPIO17, GPIO18, GPIO19, GPIO33, GPIO55, LCD_CAM},
    time::Rate,
};

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 480;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * 2;
/// A complete frame has separate spare capacity for the FIFO's final burst.
const TAIL_BYTES: usize = 64;

pub struct Dvp {
    camera: Option<Camera<'static, Async>>,
    buffer: Option<DmaRxBuf>,
    data2: Flex<'static>,
    serial_pad: Option<(bool, bool, u32, bool)>,
    vsync: Option<Input<'static>>,
}
impl Dvp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lcd: LCD_CAM<'static>,
        dma: DMA_AXI_CH2<'static>,
        d0: GPIO16<'static>,
        d1: GPIO15<'static>,
        d2: GPIO33<'static>,
        d6: GPIO18<'static>,
        vsync: GPIO55<'static>,
        href: GPIO19<'static>,
        pclk: GPIO17<'static>,
    ) -> Self {
        // The sensor supplies XCLK. This is the LCD_CAM sampling clock and
        // must exceed the external PCLK so input synchronization loses no edges.
        // esp_driver_cam inverts VSYNC at the GPIO matrix. Apply the same
        // polarity in LCD_CAM: the sensor's falling edge ends active lines.
        let config = Config::default()
            .with_frequency(Rate::from_mhz(80))
            .with_eof_mode(EofMode::VsyncSignal)
            .with_invert_vsync(true);
        let vsync = Input::new(vsync, InputConfig::default());
        let camera = Camera::new(LcdCam::new(lcd).cam, dma, config)
            .expect("camera LCD_CAM config")
            .with_data0(d0)
            .with_data1(d1)
            .with_data6(d6)
            .with_vsync(vsync.peripheral_input())
            .with_h_enable(href)
            .with_pixel_clock(pclk)
            .into_async();
        Self {
            camera: Some(camera),
            buffer: None,
            data2: Flex::new(d2),
            serial_pad: None,
            vsync: Some(vsync),
        }
    }
    pub fn start(&mut self, data: [InputSignal<'static>; 4]) -> Result<(), &'static str> {
        self.claim_serial_pad();
        let [d3, d4, d5, d7] = data;
        self.camera = Some(
            self.camera
                .take()
                .ok_or("camera-hardware-busy")?
                .with_data2(self.data2.peripheral_input())
                .with_data3(d3)
                .with_data4(d4)
                .with_data5(d5)
                .with_data7(d7),
        );
        if self.buffer.is_none() {
            // Allocated once and reused on every start/stop. Descriptors stay
            // in internal RAM; the data allocation has independent cachelines.
            let memory = Box::leak(Box::new(PixelMemory::new(FRAME_BYTES + TAIL_BYTES, true)));
            let bytes = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), memory.len()) };
            const COUNT: usize = (((FRAME_BYTES + TAIL_BYTES).div_ceil(4000)) + 3) & !3;
            static DESCRIPTORS: static_cell::ConstStaticCell<
                InternalMemory<[DmaDescriptor; COUNT]>,
            > = static_cell::ConstStaticCell::new(InternalMemory::new(
                [DmaDescriptor::EMPTY; COUNT],
            ));
            let descriptors = DESCRIPTORS.take().get_mut().unsize();
            let aligned = DmaAlignedMut::new(bytes).map_err(|_| "camera-dma-alignment")?;
            self.buffer =
                Some(DmaRxBuf::new(descriptors, aligned).map_err(|_| "camera-dma-buffer")?);
        }
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<usize, &'static str> {
        let camera = self.camera.take().ok_or("camera-hardware-busy")?;
        let buffer = self.buffer.take().ok_or("camera-buffer-unavailable")?;
        let vsync = self.vsync.take().ok_or("camera-sync-unavailable")?;
        WORK.send(Work {
            camera,
            buffer,
            vsync,
        })
        .await;
        let (work, result) = DONE.receive().await;
        self.camera = Some(work.camera);
        self.buffer = Some(work.buffer);
        self.vsync = Some(work.vsync);
        result
    }
    pub fn pixels(&self) -> &[u8] {
        &self.buffer.as_ref().expect("idle camera buffer").as_slice()[..FRAME_BYTES]
    }
    pub fn stop(&mut self) {
        self.restore_serial_pad();
    }
    fn claim_serial_pad(&mut self) {
        if self.serial_pad.is_some() {
            return;
        }
        // USB_DEVICE is Serial/JTAG, not the USB_HS CDC console.
        let usb = unsafe { &*esp32s31::USB_DEVICE::PTR };
        let clocks = unsafe { &*esp32s31::HP_SYS_CLKRST::PTR };
        let apb = clocks.usb_device_ctrl0().read().apb_clk_en().bit();
        clocks
            .usb_device_ctrl0()
            .modify(|_, w| w.apb_clk_en().set_bit());
        let pad = usb.conf0().read().usb_pad_enable().bit();
        let clock = usb.misc_conf().read().clk_en().bit();
        let interrupt = usb.int_ena().read().bits();
        self.serial_pad = Some((pad, clock, interrupt, apb));
        usb.int_ena().write(|w| unsafe { w.bits(0) });
        usb.conf0().modify(|_, w| w.usb_pad_enable().clear_bit());
        usb.misc_conf().modify(|_, w| w.clk_en().clear_bit());
        clocks
            .usb_device_ctrl0()
            .modify(|_, w| w.apb_clk_en().clear_bit());
    }
    fn restore_serial_pad(&mut self) {
        let Some((pad, clock, interrupt, apb)) = self.serial_pad.take() else {
            return;
        };
        let usb = unsafe { &*esp32s31::USB_DEVICE::PTR };
        let clocks = unsafe { &*esp32s31::HP_SYS_CLKRST::PTR };
        clocks
            .usb_device_ctrl0()
            .modify(|_, w| w.apb_clk_en().set_bit());
        usb.misc_conf().modify(|_, w| w.clk_en().bit(clock));
        usb.conf0().modify(|_, w| w.usb_pad_enable().bit(pad));
        usb.int_ena().write(|w| unsafe { w.bits(interrupt) });
        clocks
            .usb_device_ctrl0()
            .modify(|_, w| w.apb_clk_en().bit(apb));
    }
}

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
struct Work {
    camera: Camera<'static, Async>,
    buffer: DmaRxBuf,
    vsync: Input<'static>,
}
// Both executors run on core 0. A work item moves the drivers; only the
// receiver may access them until it returns the item after DMA has stopped.
unsafe impl Send for Work {}
static WORK: Channel<CriticalSectionRawMutex, Work, 1> = Channel::new();
static DONE: Channel<CriticalSectionRawMutex, (Work, Result<usize, &'static str>), 1> =
    Channel::new();

/// Only frame boundary handling runs at interrupt priority. Pixel conversion,
/// SCCB, JPEG and filesystem operations stay on the thread executor.
#[embassy_executor::task]
pub async fn receive_task() {
    loop {
        let Work {
            camera,
            buffer,
            mut vsync,
        } = WORK.receive().await;
        // The falling edge begins vertical blanking. Arm before the next
        // active lines; inverted VSYNC ends DMA at the next falling edge.
        if with_timeout(Duration::from_millis(750), vsync.wait_for_falling_edge())
            .await
            .is_err()
        {
            DONE.send((
                Work {
                    camera,
                    buffer,
                    vsync,
                },
                Err("camera-vsync-timeout"),
            ))
            .await;
            continue;
        }
        let mut transfer = match camera.receive(buffer) {
            Ok(transfer) => transfer,
            Err((_, camera, buffer)) => {
                DONE.send((
                    Work {
                        camera,
                        buffer,
                        vsync,
                    },
                    Err("camera-dma-start"),
                ))
                .await;
                continue;
            }
        };
        let result = with_timeout(Duration::from_millis(750), transfer.wait_for_dma_eof()).await;
        let (camera, buffer) = transfer.stop();
        let received = buffer.number_of_received_bytes(); // synchronizes cached descriptors/data
        let result = match result {
            Ok(Ok(())) if (FRAME_BYTES..=FRAME_BYTES + TAIL_BYTES).contains(&received) => {
                Ok(received)
            }
            Ok(Ok(())) => {
                log::info!("camera: incomplete frame bytes={received}");
                Err("camera-short-frame")
            }
            Ok(Err(error)) => {
                log::warn!("camera: DMA {error:?} bytes={received}");
                Err("camera-dma-eof")
            }
            Err(_) => {
                log::warn!("camera: timeout bytes={received}");
                Err("camera-frame-timeout")
            }
        };
        DONE.send((
            Work {
                camera,
                buffer,
                vsync,
            },
            result,
        ))
        .await;
    }
}
