//! LEFT-slot camera preview. EEPROM and sensor identity are verified before
//! claiming module signals. No JPEG, shutter, storage or synthetic frames.

pub mod protocol;

use core::{
    alloc::Layout,
    ptr, slice,
    sync::atomic::{fence, Ordering},
};
use esp_hal::{
    dma::{aligned::DmaAlignedMut, DmaDescriptor, DmaRxBuf},
    gpio::{AnyPin, DriveMode, Level, Output, OutputConfig},
    i2c::master::I2c,
    lcd_cam::{
        cam::{Camera, CameraTransfer, Config},
        LcdCam,
    },
    peripherals::{DMA_AXI_CH1, LCD_CAM, USB_DEVICE},
    Blocking,
};
use protocol::{
    CaptureAction, Error, InitRegisters, Sccb, Sensor, Session, SessionAction, CAPTURE_TIMEOUT_US,
    FRAME_BYTES, PREVIEW,
};
use slint::{Rgb8Pixel, SharedPixelBuffer};

// 64-byte tail gives VSYNC EOF room to finalize the last pixel descriptor.
// PSRAM buffer size and every descriptor chunk remain 64-byte aligned.
const CAPTURE_BYTES: usize = FRAME_BYTES + 64;
const CHUNK_BYTES: usize = 4032;
const CONVERT_ROWS: usize = 16;

/// All signals belong to the LEFT camera. GPIO12 must not be claimed by PWM.
pub struct Hardware {
    pub lcd_cam: LCD_CAM<'static>,
    pub dma: DMA_AXI_CH1<'static>,
    pub data: [AnyPin<'static>; 8],
    pub pclk: AnyPin<'static>,
    pub vsync: AnyPin<'static>,
    pub href: AnyPin<'static>,
    pub reset: AnyPin<'static>,
    pub pwdn: AnyPin<'static>,
    pub flash: AnyPin<'static>,
}
struct Signals {
    lcd_cam: LCD_CAM<'static>,
    dma: DMA_AXI_CH1<'static>,
    data: [AnyPin<'static>; 8],
    pclk: AnyPin<'static>,
    vsync: AnyPin<'static>,
    href: AnyPin<'static>,
}
struct UsbState {
    pads: [bool; 5],
    interrupts: u32,
}
struct Controls {
    reset: Output<'static>,
    pwdn: Output<'static>,
    _flash: Output<'static>,
}

impl Sccb for I2c<'static, Blocking> {
    fn write(&mut self, address: u8, data: &[u8]) -> Result<(), Error> {
        self.write(address, data).map_err(|_| Error::Bus)
    }
    fn read(&mut self, address: u8, prefix: &[u8]) -> Result<u8, Error> {
        let mut value = [0];
        self.write_read(address, prefix, &mut value)
            .map_err(|_| Error::Bus)?;
        Ok(value[0])
    }
}

enum Phase {
    Idle,
    ResetLow(u64),
    ProbeOv(u64),
    ProbeSc(u64),
    Configure(InitRegisters),
    Capture(u64),
    Convert(usize),
    Failed,
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub frames: u32,
    pub errors: u32,
    pub timeouts: u32,
    pub received: usize,
}

pub struct CameraModule {
    bus: Option<I2c<'static, Blocking>>,
    hardware: Option<Hardware>,
    signals: Option<Signals>,
    controls: Option<Controls>,
    camera: Option<Camera<'static>>,
    buffer: Option<DmaRxBuf>,
    transfer: Option<CameraTransfer<'static, DmaRxBuf>>,
    descriptors: *mut DmaDescriptor,
    descriptor_count: usize,
    pixels: *mut u8,
    sensor: Option<Sensor>,
    phase: Phase,
    session: Session,
    pending: Option<SharedPixelBuffer<Rgb8Pixel>>,
    status: &'static str,
    ready: bool,
    conflict: bool,
    // Only these USB pad fields and the interrupt-enable mask are modified.
    usb_saved: Option<UsbState>,
    stats: Stats,
    failed_attempts: u8,
}

impl CameraModule {
    #[cfg(not(feature = "fan-pwm-gpio12"))]
    pub fn new(bus: I2c<'static, Blocking>, hardware: Hardware) -> Self {
        let mut this = Self::conflict();
        this.bus = Some(bus);
        this.hardware = Some(hardware);
        this.conflict = false;
        this.status = "Open camera to start preview";
        this
    }
    /// The opt-in fan feature never configures any camera pins or bus.
    pub fn conflict() -> Self {
        Self {
            bus: None,
            hardware: None,
            signals: None,
            controls: None,
            camera: None,
            buffer: None,
            transfer: None,
            descriptors: ptr::null_mut(),
            descriptor_count: 0,
            pixels: ptr::null_mut(),
            sensor: None,
            phase: Phase::Idle,
            session: Session::default(),
            pending: None,
            status: "GPIO12 reserved for fan — camera unavailable",
            ready: false,
            conflict: true,
            usb_saved: None,
            stats: Stats::default(),
            failed_attempts: 0,
        }
    }
    pub fn preview_ready(&self) -> bool {
        self.ready
    }
    pub fn status(&self) -> &'static str {
        self.status
    }
    pub fn retry_available(&self) -> bool {
        !self.conflict && matches!(self.phase, Phase::Failed)
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Nonblocking phases except bounded SCCB transactions (100ms bus timeout).
    /// Pixels are read only after stop + HAL DMA cache synchronization.
    pub fn poll(
        &mut self,
        active: bool,
        retry: bool,
        now: u64,
    ) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
        if self.conflict {
            return None;
        }
        match self.session.update(active, retry) {
            SessionAction::Stop => {
                self.cancel();
                self.status = "Open camera to start preview";
            }
            SessionAction::Start => {
                self.cancel();
                self.begin(now);
            }
            SessionAction::None => {}
        }
        if !active {
            return None;
        }
        let phase = core::mem::replace(&mut self.phase, Phase::Idle);
        match phase {
            Phase::ResetLow(deadline) if now >= deadline => {
                self.controls.as_mut().unwrap().reset.set_high();
                self.phase = Phase::ProbeOv(now + 20_000);
            }
            Phase::ProbeOv(deadline) if now >= deadline => {
                if protocol::detect(self.bus.as_mut().unwrap(), Sensor::Ov3640).is_ok() {
                    self.configure(Sensor::Ov3640);
                } else {
                    self.controls.as_mut().unwrap().pwdn.set_high();
                    self.phase = Phase::ProbeSc(now + 20_000);
                }
            }
            Phase::ProbeSc(deadline) if now >= deadline => {
                if protocol::detect(self.bus.as_mut().unwrap(), Sensor::Sc101iot).is_ok() {
                    self.configure(Sensor::Sc101iot);
                } else {
                    self.fail("Camera sensor not recognized — Retry");
                }
            }
            Phase::Configure(mut registers) => {
                let sensor = self.sensor.unwrap();
                match registers.advance(self.bus.as_mut().unwrap(), sensor, now) {
                    Err(_) => self.fail("Camera initialization failed — Retry"),
                    Ok(false) => self.phase = Phase::Configure(registers),
                    Ok(true) => {
                        if self.prepare_capture().is_err() || self.stream(true).is_err() {
                            self.fail("Camera initialization failed — Retry");
                        } else {
                            self.arm(now);
                        }
                    }
                }
            }
            Phase::Capture(started) => {
                let eof = self.first_eof();
                let stopped = self.transfer.as_ref().unwrap().is_done();
                if eof.is_some()
                    || stopped
                    || self.dma_error()
                    || now.saturating_sub(started) >= CAPTURE_TIMEOUT_US
                {
                    let (camera, buffer) = self.transfer.take().unwrap().stop();
                    // This API invalidates descriptors AND PSRAM. as_slice() alone does not.
                    let received = buffer.number_of_received_bytes();
                    self.stats.received = received;
                    self.camera = Some(camera);
                    self.buffer = Some(buffer);
                    let synced_eof = self.first_eof();
                    match protocol::capture_action(
                        received,
                        synced_eof,
                        self.dma_error(),
                        now.saturating_sub(started),
                        self.failed_attempts,
                    ) {
                        CaptureAction::Convert => {
                            self.failed_attempts = 0;
                            self.pending =
                                Some(SharedPixelBuffer::new(PREVIEW as u32, PREVIEW as u32));
                            self.phase = Phase::Convert(0);
                        }
                        CaptureAction::Discard => {
                            self.failed_attempts += 1;
                            log::warn!(
                                "相机丢弃短帧: {received}B EOF={synced_eof:?} 重捕={}",
                                self.failed_attempts
                            );
                            self.arm(now);
                        }
                        CaptureAction::Timeout => {
                            self.stats.timeouts += 1;
                            self.fail("Camera frame timed out — Retry");
                        }
                        CaptureAction::DmaError => self.fail("Camera DMA failed — Retry"),
                        CaptureAction::Incomplete => self.fail("Camera frame incomplete — Retry"),
                    }
                } else {
                    self.phase = Phase::Capture(started);
                }
            }
            Phase::Convert(row) => {
                let end = (row + CONVERT_ROWS).min(PREVIEW);
                let result = protocol::convert_rows(
                    self.sensor.unwrap(),
                    self.buffer.as_ref().unwrap().as_slice(),
                    self.pending.as_mut().unwrap().make_mut_bytes(),
                    row,
                    end,
                );
                if result.is_err() {
                    self.fail("Camera conversion failed — Retry");
                } else if end == PREVIEW {
                    self.ready = true;
                    self.stats.frames += 1;
                    self.status = match self.sensor.unwrap() {
                        Sensor::Ov3640 => "OV3640 · Preview only",
                        Sensor::Sc101iot => "SC101IOT · Preview only",
                    };
                    let image = self.pending.take();
                    self.arm(now);
                    return image;
                } else {
                    self.phase = Phase::Convert(end);
                }
            }
            other => self.phase = other,
        }
        None
    }

    fn begin(&mut self, now: u64) {
        self.ready = false;
        self.sensor = None;
        self.failed_attempts = 0;
        self.status = "Checking left camera module…";
        let mut descriptor = [0; 0x86];
        if self
            .bus
            .as_mut()
            .unwrap()
            .write_read(0x50u8, &[0], &mut descriptor)
            .is_err()
        {
            self.fail("No camera in left slot — Retry");
            return;
        }
        match protocol::camera_descriptor(&descriptor) {
            Err(Error::WrongModule) => {
                self.fail("Left module is not a camera — Retry");
                return;
            }
            Err(_) => {
                self.fail("Camera module descriptor invalid — Retry");
                return;
            }
            Ok(()) => {}
        }
        // GPIO34 (flash) is USJ_DP as well as GPIO33 (D2) being USJ_DM.
        // Save USB state before Output::new can disable either pad implicitly.
        self.acquire_usb();
        if let Some(hw) = self.hardware.take() {
            self.controls = Some(Controls {
                reset: Output::new(hw.reset, Level::Low, OutputConfig::default()),
                pwdn: Output::new(hw.pwdn, Level::Low, OutputConfig::default()),
                _flash: Output::new(
                    hw.flash,
                    Level::High,
                    OutputConfig::default().with_drive_mode(DriveMode::OpenDrain),
                ),
            });
            self.signals = Some(Signals {
                lcd_cam: hw.lcd_cam,
                dma: hw.dma,
                data: hw.data,
                pclk: hw.pclk,
                vsync: hw.vsync,
                href: hw.href,
            });
        }
        self.sensor = None;
        let controls = self.controls.as_mut().unwrap();
        controls.pwdn.set_low();
        controls.reset.set_low();
        self.phase = Phase::ResetLow(now + 10_000);
    }
    fn configure(&mut self, sensor: Sensor) {
        self.sensor = Some(sensor);
        self.status = "Initializing camera…";
        log::info!(
            "左槽相机检测: {} @0x{:02X}",
            sensor.name(),
            sensor.address()
        );
        self.phase = Phase::Configure(InitRegisters::default());
    }
    fn prepare_capture(&mut self) -> Result<(), Error> {
        self.acquire_usb();
        if let Some(signals) = self.signals.take() {
            let [d0, d1, d2, d3, d4, d5, d6, d7] = signals.data;
            let camera = Camera::new(
                LcdCam::new(signals.lcd_cam).cam,
                signals.dma,
                Config::default(),
            )
            .map_err(|_| Error::Bus)?
            .with_pixel_clock(signals.pclk)
            .with_vsync(signals.vsync)
            .with_h_enable(signals.href)
            .with_data0(d0)
            .with_data1(d1)
            .with_data2(d2)
            .with_data3(d3)
            .with_data4(d4)
            .with_data5(d5)
            .with_data6(d6)
            .with_data7(d7);
            self.camera = Some(camera);
        }
        // A fixed HAL configuration failure must remain a reported failure,
        // including subsequent retries, rather than unwrapping a missing Camera.
        if self.camera.is_none() {
            return Err(Error::Bus);
        }
        if self.buffer.is_none() {
            if self.pixels.is_null() {
                let layout = Layout::from_size_align(CAPTURE_BYTES, 128).unwrap();
                // One lifetime-long reusable PSRAM allocation, never into a Slint image.
                self.pixels = unsafe {
                    esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout)
                };
                if self.pixels.is_null() {
                    return Err(Error::Bus);
                }
            }
            if self.descriptors.is_null() {
                // StaticCell is taken once. Even a buffer validation failure reuses
                // this allocation on retry, rather than taking it a second time.
                let (descriptors, _) =
                    esp_hal::dma_descriptors_chunk_size!(CAPTURE_BYTES, 0, CHUNK_BYTES);
                self.descriptors = descriptors.as_mut_ptr();
                self.descriptor_count = descriptors.len();
            }
            // No buffer/transfer exists in this branch. Any failed constructor
            // has released its borrows, so these uniquely owned allocations may
            // be reborrowed; a successful DmaRxBuf owns them until stop returns it.
            let pixels = unsafe { slice::from_raw_parts_mut(self.pixels, CAPTURE_BYTES) };
            let descriptors =
                unsafe { slice::from_raw_parts_mut(self.descriptors, self.descriptor_count) };
            self.buffer = Some(
                DmaRxBuf::new(
                    DmaAlignedMut::new(descriptors).map_err(|_| Error::Bus)?,
                    DmaAlignedMut::new(pixels).map_err(|_| Error::Bus)?,
                )
                .map_err(|_| Error::Bus)?,
            );
        }
        Ok(())
    }
    fn arm(&mut self, now: u64) {
        // prepare_transfer resets every length/EOF flag before ownership passes to DMA.
        match self
            .camera
            .take()
            .unwrap()
            .receive(self.buffer.take().unwrap())
        {
            Ok(transfer) => {
                self.transfer = Some(transfer);
                self.phase = Phase::Capture(now);
            }
            Err((error, camera, buffer)) => {
                self.camera = Some(camera);
                self.buffer = Some(buffer);
                log::warn!("相机 DMA 启动失败: {error:?}");
                self.fail("Camera DMA failed — Retry");
            }
        }
    }
    fn first_eof(&self) -> Option<usize> {
        let mut total = 0;
        for index in 0..self.descriptor_count {
            // S31 descriptors are allocated by the HAL macro in internal uncached
            // DRAM (soc_internal_memory_cached is false). Pixel PSRAM is cached;
            // it is never read here. Only the hardware-written 32-bit flags word
            // is volatile; inspect data only once CPU ownership and EOF arrive.
            let flags =
                unsafe { ptr::read_volatile(ptr::addr_of!((*self.descriptors.add(index)).flags)) };
            if flags.owner() {
                break;
            }
            fence(Ordering::Acquire);
            let length = flags.length() as usize;
            if length > flags.size() as usize || total + length > CAPTURE_BYTES {
                return Some(0);
            }
            total += length;
            if flags.suc_eof() {
                return Some(total);
            }
        }
        None
    }
    fn dma_error(&self) -> bool {
        // CameraTransfer::is_done is FIFO/DMA stopping, not a success flag.
        // HAL stop() deliberately discards DMA result; check CH1's error status.
        let raw = esp_hal::peripherals::AXI_GDMA::regs()
            .in_ch(1)
            .in_int()
            .raw()
            .read();
        raw.in_dscr_err().bit_is_set() || raw.in_err_eof().bit_is_set()
    }
    fn stream(&mut self, enabled: bool) -> Result<(), Error> {
        let Some(sensor) = self.sensor else {
            return Ok(());
        };
        let bus = self.bus.as_mut().unwrap();
        match sensor {
            Sensor::Ov3640 => {
                protocol::write_register(bus, sensor, 0x3086, if enabled { 0 } else { 3 })
            }
            Sensor::Sc101iot => {
                let current = protocol::read_register(bus, sensor, 0x3100)?;
                protocol::write_register(bus, sensor, 0x3100, (current & !1) | u8::from(enabled))
            }
        }
    }
    fn cancel(&mut self) {
        if let Some(transfer) = self.transfer.take() {
            let (camera, buffer) = transfer.stop();
            self.camera = Some(camera);
            self.buffer = Some(buffer);
        }
        self.pending = None;
        self.ready = false;
        if let Err(error) = self.stream(false) {
            log::warn!("相机暂停 SCCB 失败: {error:?}");
        }
        if let Some(controls) = self.controls.as_mut() {
            match self.sensor {
                Some(Sensor::Ov3640) => controls.pwdn.set_high(),
                Some(Sensor::Sc101iot) => controls.pwdn.set_low(),
                // OV held in reset and SC powered down when identity is unknown.
                None => {
                    controls.reset.set_low();
                    controls.pwdn.set_low();
                }
            }
        }
        self.release_usb();
        self.phase = Phase::Idle;
    }
    fn fail(&mut self, status: &'static str) {
        self.stats.errors += 1;
        self.cancel();
        self.phase = Phase::Failed;
        self.status = status;
        log::warn!("相机: {status}, 收到={}B", self.stats.received);
    }
    fn acquire_usb(&mut self) {
        if self.usb_saved.is_none() {
            let regs = USB_DEVICE::regs();
            let old = regs.conf0().read();
            self.usb_saved = Some(UsbState {
                pads: [
                    old.usb_pad_enable().bit(),
                    old.dm_pullup().bit(),
                    old.dm_pulldown().bit(),
                    old.dp_pullup().bit(),
                    old.dp_pulldown().bit(),
                ],
                interrupts: regs.int_ena().read().bits(),
            });
            regs.int_ena().write(|w| unsafe { w.bits(0) });
            regs.conf0().modify(|_, w| {
                w.usb_pad_enable().clear_bit();
                w.dm_pullup().clear_bit();
                w.dm_pulldown().clear_bit();
                w.dp_pullup().clear_bit();
                w.dp_pulldown().clear_bit()
            });
        }
    }
    fn release_usb(&mut self) {
        if let Some(old) = self.usb_saved.take() {
            let regs = USB_DEVICE::regs();
            // Restore only pad fields, preserving other USB configuration.
            regs.conf0().modify(|_, w| {
                w.usb_pad_enable().bit(old.pads[0]);
                w.dm_pullup().bit(old.pads[1]);
                w.dm_pulldown().bit(old.pads[2]);
                w.dp_pullup().bit(old.pads[3]);
                w.dp_pulldown().bit(old.pads[4])
            });
            regs.int_ena().write(|w| unsafe { w.bits(old.interrupts) });
        }
    }
}
