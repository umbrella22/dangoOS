//! ESP-Mosaico v1.2 S31 firmware entry point.
//!
//! `main` brings up the board and spawns the tasks; everything else lives in
//! modules:
//!
//! - [`board`]: the v1.2 pin map and board-revision check.
//! - [`drivers`]: panel, touch, sensors, fuel gauge, expansion slots, LVGL port.
//! - [`tasks`]: the Embassy tasks (see below).
//! - [`console`]: the USB CDC-ACM log console and app push protocol.
//! - The host-testable library (`os`): behaviour, fox, UI, telemetry.
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

// Existing devices validate this OTA identity; Cargo package names may change.
esp_bootloader_esp_idf::esp_app_desc!(
    env!("CARGO_PKG_VERSION"),
    "mosaico-s31",
    esp_bootloader_esp_idf::BUILD_TIME,
    esp_bootloader_esp_idf::BUILD_DATE,
    esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
    esp_bootloader_esp_idf::MMU_PAGE_SIZE,
    0,
    u16::MAX,
    esp_bootloader_esp_idf::SECURE_VERSION
);

// `board.rs` carries its own `#![allow(dead_code)]`: the pin map is
// documentation.
mod board;
mod console;
mod crashlog;
mod drivers;
mod ota_runtime;
mod tasks;
#[cfg(feature = "usb-session-diagnostics")]
mod usb_diag_driver;

use embassy_executor::Spawner;
use embassy_futures::join::join4;
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State},
    Builder,
};
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
#[cfg(feature = "usb-session-diagnostics")]
use os::telemetry::usb_session as usb_diag;

use crate::drivers::{display, shared_i2c, subboard, touch};
use crate::tasks::{haptic, input, lights, lua, plane, sensors, ui};

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    ota_runtime::record_boot_attempt();
    #[cfg(feature = "usb-session-diagnostics")]
    usb_diag::begin_boot();
    console::install_logger();
    crashlog::report_last_boot();

    info!("==============================================");
    info!("Mosaico S31 R1 platform runtime");
    info!("scheduler: esp-rtos + Embassy (thread + interrupt executor)");
    info!("display:   async QSPI fox plane, two-slot canvas pool");
    info!("input:     GPIO6 IRQ + async I2C CST9220");
    info!("console:   USB-OTG HS CDC-ACM (USB-C), buffered log bridge");
    info!("==============================================");

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    drivers::storage::init_flash();
    ota_runtime::early_boot();
    // LVGL plus the 480x48 internal-RAM draw buffer need more than the R0
    // heartbeat-only heap; this matches the archived UI firmware.
    esp_alloc::heap_allocator!(size: 256 * 1024);
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // The flash file systems are mounted once USB is up (see the end), so a
    // failure there shows up on the console.
    let flash = peripherals.FLASH;

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
    #[cfg(feature = "usb-session-diagnostics")]
    spawner.spawn(usb_diagnostic_sampler().unwrap());

    // BSP power switch is open-drain, active low: high means released, never
    // push-pull high. Inputs are low-active with pull-ups, including BOOT.
    let mut shutdown = Output::new(
        peripherals.GPIO57,
        Level::High,
        OutputConfig::default().with_drive_mode(esp_hal::gpio::DriveMode::OpenDrain),
    );
    shutdown.set_pad_hold(false);
    let button_config = esp_hal::gpio::InputConfig::default().with_pull(esp_hal::gpio::Pull::Up);
    let ai = esp_hal::gpio::Input::new(peripherals.GPIO7, button_config);
    let boot = esp_hal::gpio::Input::new(peripherals.GPIO61, button_config);
    spawner.spawn(tasks::buttons::button_task(ai, boot, shutdown).unwrap());

    // The console comes up first so that later boot steps can be seen. After
    // a panic, wait (bounded) for the host so the record is read before the
    // same step can fail again.
    spawner.spawn(usb_task(peripherals.USB_HS, flash).unwrap());
    // WS2812 retains its last colour across an MCU reset. Clear it before
    // slow panel/storage work or the bounded post-panic USB wait. This only
    // runs after firmware entry; ROM download mode cannot execute this clear.
    // Haptics: motor on GPIO8 (BSP BSP_MOTOR_GPIO, active high). The S31
    // LEDC driver is not available in this esp-hal revision; the RMT TX
    // peripheral generates the same 5 kHz PWM in hardware.
    // RMT runs at 10 MHz: channel 0 divides to 1 MHz for the motor PWM,
    // channel 1 keeps 0.1 us ticks for the WS2812 light on GPIO38.
    match Rmt::new(peripherals.RMT, Rate::from_mhz(10)) {
        Ok(rmt) => {
            match haptic::configure_channel(rmt.channel0) {
                Ok(channel) => {
                    spawner
                        .spawn(haptic::haptic_task(channel.with_pin(peripherals.GPIO8)).unwrap());
                    info!("haptic task spawned (RMT TX 5 kHz PWM on GPIO8)");
                }
                Err(error) => {
                    warn!("haptic channel setup failed: {error:?}; motor held low");
                    let _motor_off =
                        Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());
                }
            }
            match lights::configure_channel(rmt.channel1) {
                Ok(channel) => {
                    spawner
                        .spawn(lights::lights_task(channel.with_pin(peripherals.GPIO38)).unwrap());
                }
                Err(error) => warn!("light channel setup failed: {error:?}; no LED clear receipt"),
            }
        }
        Err(error) => {
            warn!("RMT initialization failed: {error:?}; haptics disabled, no LED clear receipt");
            let _motor_off = Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());
        }
    }

    let light_clear_deadline =
        embassy_time::Instant::now() + embassy_time::Duration::from_millis(100);
    while !lights::startup_cleared() && embassy_time::Instant::now() < light_clear_deadline {
        embassy_time::Timer::after_millis(1).await;
    }
    if !lights::startup_cleared() {
        warn!("lights: early startup clear unavailable after 100ms; continuing USB startup");
    }

    if crashlog::last_boot_panicked() {
        let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_secs(5);
        while !console::connected() && embassy_time::Instant::now() < deadline {
            embassy_time::Timer::after_millis(50).await;
        }
        embassy_time::Timer::after_millis(300).await;
    }

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

    spawner.spawn(lua::lua_task().unwrap());
    info!("lua task spawned ({})", lua_engine::LUA_RELEASE);

    // Everything else runs in the spawned tasks.
    core::future::pending::<()>().await;
}

/// Mount `/system` and `/data` once the console is up (the UI task scans the
/// apps when this is done).
async fn mount_storage(flash: esp_hal::peripherals::FLASH<'static>) {
    embassy_time::Timer::after_millis(1500).await;
    if crashlog::skip(crashlog::Stage::Storage) {
        warn!("fs: not mounted this boot (the last one panicked there); built-in apps only");
    } else {
        crashlog::enter(crashlog::Stage::Storage);
        drivers::storage::init(flash);
        crashlog::enter(crashlog::Stage::Running);
    }
    drivers::storage::mark_ready();
}

/// Leak `val` into a `'static` slot (one call site, called once).
macro_rules! make_static {
    ($t:ty, $val:expr) => {{
        static mut CELL: core::mem::MaybeUninit<$t> = core::mem::MaybeUninit::uninit();
        // SAFETY: the enclosing function runs once (it is a spawned task
        // with a pool size of one), so this is the only reference.
        #[allow(static_mut_refs)]
        unsafe {
            CELL.write($val)
        }
    }};
}

/// USB-OTG HS CDC-ACM console on the USB-C port (logs out, app push in), and
/// the flash file systems once the console is up.
#[embassy_executor::task]
async fn usb_task(
    usb_hs: esp_hal::peripherals::USB_HS<'static>,
    flash: esp_hal::peripherals::FLASH<'static>,
) {
    // The board does not expose USB-Serial-JTAG on that connector, so this
    // is the only log path.
    let usb = Usb::new_hs(usb_hs);
    let ep_out_buffer = make_static!([u8; 1024], [0u8; 1024]);
    let driver = Driver::new(usb, ep_out_buffer, UsbDriverConfig::default());
    #[cfg(feature = "usb-session-diagnostics")]
    let driver = usb_diag_driver::Driver(driver);

    let mut usb_config = embassy_usb::Config::new(0x303A, 0x4001);
    usb_config.max_packet_size_0 = 64;
    usb_config.manufacturer = Some("Mosaico");
    usb_config.product = Some("Mosaico S31 R1 console");
    usb_config.serial_number = Some("mosaico-s31-r1");

    let config_descriptor = make_static!([u8; 256], [0u8; 256]);
    let bos_descriptor = make_static!([u8; 256], [0u8; 256]);
    let control_buf = make_static!([u8; 64], [0u8; 64]);
    let cdc_state = make_static!(State<'static>, State::new());

    let mut builder = Builder::new(
        driver,
        usb_config,
        config_descriptor,
        bos_descriptor,
        &mut [],
        control_buf,
    );
    #[cfg(feature = "usb-session-diagnostics")]
    builder.handler(make_static!(usb_diag::Handler, usb_diag::Handler));
    // High-speed CDC requires a 512-byte packet size.
    let class = CdcAcmClass::new(&mut builder, cdc_state, 512);
    let (mut tx, mut rx) = class.split();
    let mut usb_device = builder.build();

    info!("USB-OTG HS CDC console starting (512-byte packets, app push)");
    join4(
        usb_device.run(),
        console::pump(&mut tx),
        console::receive(&mut rx),
        mount_storage(flash),
    )
    .await;
}

#[cfg(feature = "usb-session-diagnostics")]
#[embassy_executor::task]
async fn usb_diagnostic_sampler() {
    use core::sync::atomic::Ordering;
    loop {
        embassy_time::Timer::after_secs(1).await;
        usb_diag::sample(
            ui::HEARTBEATS.load(Ordering::Relaxed),
            os::telemetry::display::FRAMES.load(Ordering::Relaxed),
            console::diagnostic_log_drops(),
        );
        if usb_diag::reset_due(embassy_time::Instant::now().as_millis()) {
            esp_hal::system::software_reset();
        }
    }
}
