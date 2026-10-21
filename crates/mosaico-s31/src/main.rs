//! ESP-Mosaico v1.2 S31 firmware entry point.
//!
//! `main` brings up the board and spawns the tasks; everything else lives in
//! modules:
//!
//! - [`board`]: the v1.2 pin map and board-revision check.
//! - [`drivers`]: panel, touch, sensors, fuel gauge, expansion slots, LVGL port.
//! - [`tasks`]: the Embassy tasks (see below).
//! - [`console`]: the USB CDC-ACM log console.
//! - The host-testable library (`mosaico_s31`): behaviour, fox, UI, telemetry.
//!
//! `esp-rtos` + Embassy, two executors:
//!
//! - **Interrupt executor (FROM_CPU_INTR1, priority 2)**
//!   - `tasks::plane`: exclusively owns the CO5300 panel. Takes the newest
//!     rendered canvas from the two-slot canvas pool and uploads it in 32-row
//!     DMA chunks. Running here lets each chunk resume as soon as its DMA
//!     completes, instead of waiting for the thread-mode renderer to yield.
//! - **Thread-mode executor**
//!   - `tasks::ui`: exclusively owns LVGL. A 5 ms loop drains sensors and
//!     touch into the behaviour runtime, renders the fox into a free canvas
//!     slot and hands each new frame to the plane.
//!   - `tasks::input`: exclusively owns the CST9220. GPIO6 falling edges plus a
//!     5 ms fallback trigger async I2C report reads into a bounded queue.
//!   - `tasks::sensors`: polls the BMI270, the BMM150 pair and both expansion
//!     slots (Interaction board on either side), then publishes `SensorEvent` values into a bounded
//!     capacity-64 channel. It shares I2C0 with touch through
//!     `drivers::shared_i2c`; the slot EEPROMs use the separate I2C1.
//!   - `tasks::haptic`: non-blocking motor patterns on GPIO8. esp-hal has no
//!     S31 LEDC driver at this revision, so RMT TX generates the equivalent
//!     5 kHz PWM in hardware.
//!
//! The console is a USB-OTG HS CDC-ACM device on the USB-C port. The boot
//! summary is replayed on every connection and includes display readiness,
//! presented frames, touch-to-first-pixel P50/P95/max, transfer time, FPS,
//! input queue depth, input drops and UI heartbeats.

#![no_std]
#![no_main]

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

// `board.rs` carries its own `#![allow(dead_code)]`: the pin map is
// documentation.
mod board;
mod console;
mod drivers;
mod tasks;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State},
    Builder,
};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    i2c::master::I2c,
    interrupt::Priority,
    rmt::Rmt,
    time::Rate,
    timer::timg::TimerGroup,
    usb::otg::{
        embassy_usb_device::{Config as UsbDriverConfig, Driver},
        Usb,
    },
};
use esp_rtos::embassy::InterruptExecutor;
use log::{info, warn};

use crate::drivers::{display, shared_i2c, subboard, touch};
use crate::tasks::{haptic, input, plane, sensors, ui};

#[unsafe(no_mangle)]
fn custom_halt() -> ! {
    esp_hal::delay::Delay::new().delay_millis(500);
    esp_hal::system::software_reset()
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    console::install_logger();

    info!("==============================================");
    info!("Mosaico S31 R1 platform runtime");
    info!("scheduler: esp-rtos + Embassy (thread + interrupt executor)");
    info!("display:   async QSPI fox plane, two-slot canvas pool");
    info!("input:     GPIO6 IRQ + async I2C CST9220");
    info!("console:   USB-OTG HS CDC-ACM (USB-C), buffered log bridge");
    info!("==============================================");

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // LVGL plus the 480x48 internal-RAM draw buffer need more than the R0
    // heartbeat-only heap; this matches the archived UI firmware.
    esp_alloc::heap_allocator!(size: 256 * 1024);
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    match board::board_variant() {
        Ok((major, minor)) => info!("board v{major}.{minor} (ESP-Mosaico v1.2 pin map)"),
        Err(v) => panic!("unsupported board v{}.{}", v.major, v.minor),
    }

    // VCC_3V3 is active-low: drive it low before panel/touch init.
    let _vcc_pw = Output::new(peripherals.GPIO60, Level::Low, OutputConfig::default());

    // The scheduler needs a timer and the FROM_CPU0 software interrupt. It must
    // be running before the first async I2C transaction uses `embassy-time`.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    info!("esp-rtos scheduler started (TIMG0 timer, FROM_CPU_INTR0)");

    // CO5300 QSPI panel: SPI2 + AXI DMA, v1.2 pin map. The vendor init sequence
    // runs in blocking mode; the driver switches to `SpiDma<Async>` afterwards.
    let panel = display::init_async(
        peripherals.SPI2,
        peripherals.DMA_AXI_CH0,
        peripherals.GPIO42,
        peripherals.GPIO36,
        peripherals.GPIO51,
        peripherals.GPIO35,
        peripherals.GPIO9,
        peripherals.GPIO50,
        peripherals.GPIO44,
    );

    // Main I2C0: CST9220 at 0x5A (SDA=56, SCL=3, 400 kHz), async.
    let i2c = I2c::new(
        peripherals.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("I2C0 initialization failed")
    .with_sda(peripherals.GPIO56)
    .with_scl(peripherals.GPIO3)
    .into_async();
    shared_i2c::install_async(i2c);
    info!("shared I2C0 installed (async, CST9220 @0x5A)");

    let touch = touch::Cst9220::new().await;
    info!("touch available={}", touch.is_some());

    let gpio6 = input::touch_irq_input(peripherals.GPIO6);

    // CO5300 tearing-effect output (GPIO43). The panel's TE signal marks the
    // start of a frame; `plane_task` starts animation transfers on it so
    // an animated frame is not split by a scanout boundary.
    let panel_te =
        esp_hal::gpio::Input::new(peripherals.GPIO43, esp_hal::gpio::InputConfig::default());

    let (plane_ui, plane_display) = plane::new();

    // The panel upload runs on a high-priority interrupt executor so its DMA
    // chunk chain preempts the thread-mode rasterizer.
    // Leaked once at boot: the executor must live for the whole run.
    let plane_executor: &'static mut InterruptExecutor<1> = alloc::boxed::Box::leak(
        alloc::boxed::Box::new(InterruptExecutor::new(peripherals.FROM_CPU_INTR1)),
    );
    let plane_spawner = plane_executor.start(Priority::Priority2);
    // SAFETY: single-core firmware; the interrupt executor runs on core 0,
    // the same core that created the panel driver.
    let panel = unsafe { plane::PanelOwner::new(panel) };
    plane_spawner.spawn(plane::plane_task(panel, plane_display, panel_te).unwrap());
    info!("plane on interrupt executor (FROM_CPU_INTR1, priority 2)");
    spawner.spawn(ui::ui_task(plane_ui).unwrap());
    match touch {
        Some(touch) => {
            spawner.spawn(input::input_task(gpio6, touch).unwrap());
            info!("R1 tasks spawned: plane, ui, input");
        }
        None => info!("R1 tasks spawned: plane, ui (input task disabled)"),
    }

    // v1.2 subboard bus (I2C1, SDA=GPIO0/SCL=GPIO1, internal pull-ups). It is
    // separate from I2C0, so the 250 ms slot scan cannot contend with touch.
    let subboard_i2c = I2c::new(
        peripherals.I2C1,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("subboard I2C1 initialization failed")
    .with_sda(peripherals.GPIO0)
    .with_scl(peripherals.GPIO1)
    .into_async();
    shared_i2c::install_subboard_async(subboard_i2c);

    // Expansion slots: EEPROM address pins (left GPIO14 low -> 0x50, right
    // GPIO39 high -> 0x51) and the Interaction board pins of each side. The
    // pins stay high-impedance until a slot's EEPROM reports an Interaction
    // board (see `drivers::subboard`).
    let subboard = subboard::Subboard::new(
        peripherals.GPIO14,
        peripherals.GPIO39,
        subboard::InteractionPort::new(
            peripherals.GPIO53,
            peripherals.ADC2,
            peripherals.GPIO4,
            peripherals.GPIO13,
            peripherals.GPIO12,
        ),
        subboard::InteractionPort::new(
            peripherals.GPIO46,
            peripherals.ADC1,
            peripherals.GPIO5,
            peripherals.GPIO11,
            peripherals.GPIO10,
        ),
    );
    spawner.spawn(sensors::sensor_task(subboard).unwrap());
    info!(
        "sensor task spawned (BMI270/BMM150 on I2C0, slot EEPROMs 0x50/0x51 on I2C1, queue={})",
        sensors::SENSOR_QUEUE_CAPACITY
    );

    // Haptics: motor on GPIO8 (BSP BSP_MOTOR_GPIO, active high). The S31
    // LEDC driver is not available in this esp-hal revision; the RMT TX
    // peripheral generates the same 5 kHz PWM in hardware.
    match Rmt::new(peripherals.RMT, Rate::from_mhz(1)) {
        Ok(rmt) => match haptic::configure_channel(rmt) {
            Ok(channel) => {
                spawner.spawn(haptic::haptic_task(channel.with_pin(peripherals.GPIO8)).unwrap());
                info!("haptic task spawned (RMT TX 5 kHz PWM on GPIO8)");
            }
            Err(error) => {
                warn!("haptic channel setup failed: {error:?}; motor held low");
                let _motor_off =
                    Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());
            }
        },
        Err(error) => {
            warn!("RMT initialization failed: {error:?}; haptics disabled");
            let _motor_off = Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());
        }
    }

    // USB-OTG HS CDC-ACM console on the USB-C port. The board does not expose
    // USB-Serial-JTAG on that connector, so this is the only log path.
    let usb = Usb::new_hs(peripherals.USB_HS);
    let mut ep_out_buffer = [0u8; 1024];
    let driver = Driver::new(usb, &mut ep_out_buffer, UsbDriverConfig::default());

    let mut usb_config = embassy_usb::Config::new(0x303A, 0x4001);
    usb_config.max_packet_size_0 = 64;
    usb_config.manufacturer = Some("Mosaico");
    usb_config.product = Some("Mosaico S31 R1 console");
    usb_config.serial_number = Some("mosaico-s31-r1");

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut control_buf = [0u8; 64];
    let mut cdc_state = State::new();

    let mut builder = Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [],
        &mut control_buf,
    );
    // High-speed CDC requires a 512-byte packet size.
    let mut class = CdcAcmClass::new(&mut builder, &mut cdc_state, 512);
    let mut usb_device = builder.build();

    info!("USB-OTG HS CDC console starting (512-byte packets)");
    join(usb_device.run(), console::pump(&mut class)).await;
}
