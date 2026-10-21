//! ESP-Mosaico v1.2 S31 platform firmware (R1 + R1.5).
//!
//! `esp-rtos` + Embassy, two executors:
//!
//! - **Interrupt executor (FROM_CPU_INTR1, priority 2)**
//!   - `ball_plane_task`: exclusively owns the CO5300 panel. Takes the newest
//!     rendered canvas from the two-slot canvas pool and uploads it in 32-row
//!     DMA chunks at the panel centre. Running here lets each chunk resume as
//!     soon as its DMA completes, instead of waiting for the thread-mode
//!     rasterizer to yield.
//! - **Thread-mode executor**
//!   - `ui_task`: exclusively owns LVGL. A 5 ms loop drains sensors and touch
//!     into the runtime, advances the ball widget (engine + raster into a free
//!     canvas slot) and hands each new frame to the plane.
//!   - `input_task`: exclusively owns the CST9220. GPIO6 falling edges plus a
//!     5 ms fallback trigger async I2C report reads into a bounded queue.
//!   - `sensor_task` (R1.5): polls the BMI270, the BMM150 pair and the
//!     right-slot Interaction module, then publishes `SensorEvent` values into
//!     a bounded capacity-64 channel. It shares I2C0 with touch through
//!     `shared_i2c`; the subboard EEPROM uses the separate I2C1.
//!   - `haptic_task` (H1.3): non-blocking motor patterns on GPIO8. esp-hal has
//!     no S31 LEDC driver at this revision, so RMT TX generates the equivalent
//!     5 kHz PWM in hardware.
//!
//! The console is a USB-OTG HS CDC-ACM device on the USB-C port. The boot
//! summary is replayed on every connection and includes the R1 counters:
//! display readiness, presented frames, touch-to-first-pixel P50/P95/max, last
//! transfer time, FPS, input queue depth, input drops and UI heartbeats.

#![no_std]
#![no_main]

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

// `board.rs` carries its own crate-level `#![allow(dead_code)]` because the pin
// map is documentation.
mod ball_plane;
mod board;
mod display_stats;
#[allow(dead_code)]
mod drivers;
// The native RGB565 model is shared by the panel driver and the ball plane.
#[allow(dead_code)]
mod framebuffer;
mod haptic;
// Only `CAPACITY` is used by the firmware; the queue itself is exported by
// the host library.
#[allow(dead_code)]
mod input_queue;
mod input_task;
mod latency;
mod lvgl_port;
// Ported allocation helper.
#[allow(dead_code)]
mod memory;
mod sensor_task;
mod ui;
mod ui_task;

use core::{
    fmt::Write as _,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_executor::Spawner;
use embassy_futures::{
    join::join,
    select::{select, Either},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State},
    driver::EndpointError,
    Builder,
};
use esp_backtrace as _;
use esp_rtos::embassy::InterruptExecutor;
use esp_hal::{
    clock::CpuClock,
    interrupt::Priority,
    gpio::{Level, Output, OutputConfig},
    i2c::master::I2c,
    rmt::Rmt,
    time::Rate,
    timer::timg::TimerGroup,
    usb::otg::{
        embassy_usb_device::{Config as UsbDriverConfig, Driver},
        Usb,
    },
};
use log::{info, warn};

use crate::drivers::{display, module, shared_i2c, touch};

/// Fixed capacity of one console line, including the trailing CRLF.
const LOG_LINE_BYTES: usize = 160;
/// Text capacity leaves room for the two-byte CRLF terminator.
const LOG_TEXT_BYTES: usize = LOG_LINE_BYTES - 2;
/// Bounded log queue. New records are dropped when the CDC pump is behind.
const LOG_CHANNEL_DEPTH: usize = 32;
/// How often the console pump reports connection state and drop counters.
const CONSOLE_STATUS_PERIOD: Duration = Duration::from_secs(5);

/// One formatted log line owned by the channel.
struct LogRecord {
    len: u8,
    bytes: [u8; LOG_LINE_BYTES],
}

/// A fixed-buffer `core::fmt::Write` sink. Formatting truncates at
/// `LOG_TEXT_BYTES`; the trailing CRLF is always appended by `finish`.
struct LogLine {
    bytes: [u8; LOG_LINE_BYTES],
    len: usize,
}

impl LogLine {
    const fn new() -> Self {
        Self {
            bytes: [0; LOG_LINE_BYTES],
            len: 0,
        }
    }

    fn finish(mut self) -> LogRecord {
        self.bytes[self.len] = b'\r';
        self.bytes[self.len + 1] = b'\n';
        self.len += 2;
        LogRecord {
            len: self.len as u8,
            bytes: self.bytes,
        }
    }
}

impl core::fmt::Write for LogLine {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = LOG_TEXT_BYTES.saturating_sub(self.len);
        let count = core::cmp::min(available, text.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

/// The bounded queue between `log` producers (all tasks) and the USB pump.
static LOG_CHANNEL: Channel<CriticalSectionRawMutex, LogRecord, LOG_CHANNEL_DEPTH> = Channel::new();
/// Records dropped because the bounded queue was full. Never reset.
static LOG_DROPS: AtomicU32 = AtomicU32::new(0);

/// `log::Log` implementation that formats into a fixed 160-byte record.
struct UsbLogger;

impl log::Log for UsbLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = LogLine::new();
        let _ = write!(line, "[{}] {}", record.level(), record.args());
        if LOG_CHANNEL.try_send(line.finish()).is_err() {
            LOG_DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

static USB_LOGGER: UsbLogger = UsbLogger;

/// Draft one R1 status line for the CDC port.
fn status_record(connected: bool) -> LogRecord {
    let (samples, p50, p95, max) = latency::summary();
    let mut line = LogLine::new();
    let _ = write!(
        line,
        "[boot] R1 cdc={connected} disp={} br={} frames={} p50={p50} p95={p95} max={max} n={samples} transfer_ms={} fps={} te={}/{}/{} ty={} tew={} q={} drops={} hb={} ldrop={}",
        display_stats::FIRST_FRAME.load(Ordering::Relaxed) as u8,
        display_stats::BRIGHTNESS_RESTORED.load(Ordering::Relaxed) as u8,
        display_stats::FRAMES.load(Ordering::Relaxed),
        display_stats::LAST_TRANSFER_MS.load(Ordering::Relaxed),
        display_stats::FPS.load(Ordering::Relaxed),
        display_stats::TE_SYNCS.load(Ordering::Relaxed),
        display_stats::TE_TIMEOUTS.load(Ordering::Relaxed),
        display_stats::TE_BYPASS.load(Ordering::Relaxed),
        display_stats::TE_YIELDS.load(Ordering::Relaxed),
        display_stats::TE_WAIT_AVG_US.load(Ordering::Relaxed) / 1000,
        input_task::INPUT.len(),
        input_task::INPUT_DROPS.load(Ordering::Relaxed),
        ui_task::HEARTBEATS.load(Ordering::Relaxed),
        LOG_DROPS.load(Ordering::Relaxed),
    );
    line.finish()
}

/// Pump queued log lines to the CDC-ACM class.
///
/// Waits for the host to open the port, drains the bounded channel, and sends a
/// status line every `CONSOLE_STATUS_PERIOD` so a disconnect is detected even
/// when no logs are produced.
async fn console_pump<'a>(class: &mut CdcAcmClass<'a, Driver<'a>>) {
    loop {
        class.wait_connection().await;
        let mut next_status = embassy_time::Instant::now() + CONSOLE_STATUS_PERIOD;
        let status = status_record(true);
        let _ = class
            .write_packet(&status.bytes[..status.len as usize])
            .await;

        loop {
            let result = match select(LOG_CHANNEL.receive(), Timer::at(next_status)).await {
                Either::First(record) => {
                    class
                        .write_packet(&record.bytes[..record.len as usize])
                        .await
                }
                Either::Second(()) => {
                    next_status += CONSOLE_STATUS_PERIOD;
                    let record = status_record(true);
                    class
                        .write_packet(&record.bytes[..record.len as usize])
                        .await
                }
            };
            match result {
                Ok(()) => {}
                Err(EndpointError::Disabled) => {
                    // Cable removed or bus reset. Wait for the next connection;
                    // queued records stay bounded while disconnected.
                    break;
                }
                Err(EndpointError::BufferOverflow) => {
                    // A single record exceeded the endpoint packet size. Drop it
                    // rather than treating it as a disconnect.
                    LOG_DROPS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
fn custom_halt() -> ! {
    esp_hal::delay::Delay::new().delay_millis(500);
    esp_hal::system::software_reset()
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // Replace the default esp-println logger: its output goes to USB-Serial-JTAG,
    // which is not wired to the USB-C connector on this board. All `log`
    // records now enter the bounded channel drained by the USB CDC pump.
    let _ = log::set_logger(&USB_LOGGER);
    log::set_max_level(log::LevelFilter::Info);

    info!("==============================================");
    info!("Mosaico S31 R1 platform runtime");
    info!("scheduler: esp-rtos + Embassy (thread + interrupt executor)");
    info!("display:   async QSPI ball plane, two-slot canvas pool");
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

    // Main I2C0: CST9220 at 0x5A (SDA=56, SCL=3, 400 kHz), async for R1.
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

    let touch = touch::Cst9220Async::new_async().await;
    info!("touch available={}", touch.is_some());

    let gpio6 = input_task::touch_irq_input(peripherals.GPIO6);

    // CO5300 tearing-effect output (GPIO43). The panel's TE signal marks the
    // start of a frame; `ball_plane_task` starts animation transfers on it so
    // an animated frame is not split by a scanout boundary.
    let panel_te = esp_hal::gpio::Input::new(
        peripherals.GPIO43,
        esp_hal::gpio::InputConfig::default(),
    );

    let (plane_ui, plane_display) = ball_plane::new();

    // The panel upload runs on a high-priority interrupt executor so its DMA
    // chunk chain preempts the thread-mode rasterizer.
    // Leaked once at boot: the executor must live for the whole run.
    let plane_executor: &'static mut InterruptExecutor<1> = alloc::boxed::Box::leak(
        alloc::boxed::Box::new(InterruptExecutor::new(peripherals.FROM_CPU_INTR1)),
    );
    let plane_spawner = plane_executor.start(Priority::Priority2);
    // SAFETY: single-core firmware; the interrupt executor runs on core 0,
    // the same core that created the panel driver.
    let panel = unsafe { ball_plane::PanelOwner::new(panel) };
    plane_spawner.spawn(ball_plane::ball_plane_task(panel, plane_display, panel_te).unwrap());
    info!("ball_plane on interrupt executor (FROM_CPU_INTR1, priority 2)");
    spawner.spawn(ui_task::ui_task(plane_ui).unwrap());
    match touch {
        Some(touch) => {
            spawner.spawn(input_task::input_task(gpio6, touch).unwrap());
            info!("R1 tasks spawned: ball_plane, ui_task, input_task");
        }
        None => info!("R1 tasks spawned: ball_plane, ui_task (input task disabled)"),
    }

    // v1.2 subboard bus (I2C1, SDA=GPIO0/SCL=GPIO1, internal pull-ups). It is
    // separate from I2C0, so the 250 ms module scan cannot contend with touch.
    let subboard_i2c = I2c::new(
        peripherals.I2C1,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("subboard I2C1 initialization failed")
    .with_sda(peripherals.GPIO0)
    .with_scl(peripherals.GPIO1)
    .into_async();
    shared_i2c::install_subboard_async(subboard_i2c);

    // Right-slot EEPROM address select: GPIO39 high -> 0x51 (BSP subboard.c).
    let _right_slot_addr_select =
        Output::new(peripherals.GPIO39, Level::High, OutputConfig::default());

    // Interaction module inputs: LDR GPIO46/ADC1_CH4, PIR GPIO5, KEY_L GPIO11,
    // KEY_R GPIO10. Only the GPIO/ADC setup happens here; sampling is R1.5.
    let module = module::ModuleWatcher::new(module::ModulePins::new(
        peripherals.GPIO46,
        peripherals.ADC1,
        peripherals.GPIO5,
        peripherals.GPIO11,
        peripherals.GPIO10,
    ));
    spawner.spawn(sensor_task::sensor_task(module).unwrap());
    info!(
        "R1.5 sensor task spawned (BMI270/BMM150 on I2C0, module EEPROM 0x51 on I2C1, queue={})",
        sensor_task::SENSOR_QUEUE_CAPACITY
    );

    // H1.3 haptics: motor on GPIO8 (BSP BSP_MOTOR_GPIO, active high). The S31
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
    join(usb_device.run(), console_pump(&mut class)).await;
}
