//! Board initialization, executor wiring and startup services.

use embassy_executor::Spawner;
use embassy_futures::join::join4;
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State},
    Builder,
};
use esp_hal::{
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

#[cfg(not(feature = "dual-core-ui"))]
use crate::apps::lua::task as lua;
#[cfg(not(feature = "dual-core-ui"))]
use crate::shell::runtime as ui;
use {
    crate::platform::buses::shared_i2c, crate::platform::drivers::display,
    crate::platform::drivers::subboard, crate::platform::drivers::touch,
};
use {
    crate::services::display::plane, crate::services::feedback::haptic,
    crate::services::feedback::lights, crate::services::input::sensors,
    crate::services::input::touch as input,
};

pub async fn run(spawner: Spawner) {
    ota_runtime::record_boot_attempt();

    console::install_logger();
    crashlog::report_last_boot();

    info!("==============================================");
    info!("Mosaico S31 R1 platform runtime");
    info!("scheduler: esp-rtos + Embassy (thread + interrupt executor)");
    info!("display:   async QSPI fox plane, two-slot canvas pool");
    info!("input:     GPIO6 IRQ + async I2C CST9220");
    info!("console:   USB-OTG HS CDC-ACM (USB-C), buffered log bridge");
    info!("==============================================");

    let peripherals = esp_hal::init(
        esp_hal::Config::default()
            .with_cpu_clock(crate::platform::runtime::frequency::device::boot_clock_config()),
    );
    crate::platform::storage::nor::init_flash();
    // LVGL's internal-RAM draw band and hardware buffers need more than the
    // archived heartbeat-only heap.
    // General allocations prefer PSRAM. DMA and radio use explicit Internal
    // capabilities, retaining internal heap space for their live buffers.
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);
    // Bluedroid adds internal control state. Keep at least 32 KiB between BSS
    // and the boot stack top; both thread mode and interrupt executors use it.
    // General allocations prefer PSRAM, while DMA/controller buffers explicitly
    // request Internal capability from this heap plus the reclaimed region.
    esp_alloc::heap_allocator!(size: 160 * 1024);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);

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
    esp_rtos::start(timg0.timer0);
    let services = service_executor::start().await;
    resident_budget::init();
    info!("esp-rtos scheduler started (TIMG0 timer, FROM_CPU_INTR0)");

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
    spawner.spawn(crate::services::power::button_task::button_task(ai, boot, shutdown).unwrap());
    spawner.spawn(crate::services::power::button_task::shutdown_radio_task().unwrap());

    // The console comes up first so that later boot steps can be seen. After
    // a panic, wait (bounded) for the host so the record is read before the
    // same step can fail again.
    services.spawn(usb_task(peripherals.USB_HS, flash).unwrap());
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
    crate::services::power::battery_task::boot_check().await;
    spawner.spawn(crate::services::power::battery_task::task().unwrap());
    info!("shared I2C0 installed (async, CST9220 @0x5A)");
    if !crate::services::power::button_task::low_battery_requested() {
        ota_runtime::early_boot();
    }

    let (network, network_runner) = crate::platform::drivers::radio::wifi::network();
    spawner.spawn(
        crate::services::radio::device::wifi::network_task(network, network_runner).unwrap(),
    );
    spawner.spawn(crate::services::radio::device::wifi::task(peripherals.WIFI, network).unwrap());
    spawner.spawn(crate::services::radio::device::classic::task(peripherals.BT).unwrap());
    services.spawn(crate::services::settings::device::task().unwrap());
    services.spawn(crate::services::settings::device::preferences_task().unwrap());
    spawner.spawn(crate::services::update::task::task(network).unwrap());
    services.spawn(crate::companion::skins::device::task().unwrap());
    services.spawn(crate::apps::device::scan_task().unwrap());
    // Dedicated SPI3 + DMA1. Enable QE over SIO before claiming all four data pins.
    crate::platform::drivers::nand::init(
        peripherals.SPI3,
        peripherals.DMA_AXI_CH1,
        peripherals.GPIO20,
        peripherals.GPIO21,
        peripherals.GPIO22,
        peripherals.GPIO23,
        peripherals.GPIO24,
        peripherals.GPIO25,
    );
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

    let audio_executor = alloc::boxed::Box::leak(alloc::boxed::Box::new(InterruptExecutor::new(
        peripherals.FROM_CPU_INTR3,
    )));
    audio_executor.start(Priority::Priority2).spawn(
        crate::services::audio::task::task(
            peripherals.I2S0,
            peripherals.DMA_CH0,
            peripherals.GPIO54,
            peripherals.GPIO37,
            peripherals.GPIO49,
            peripherals.GPIO52,
            peripherals.GPIO40,
            peripherals.GPIO45,
        )
        .unwrap(),
    );

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
    // SAFETY: this interrupt executor remains on Core0, the same core that
    // created the panel driver. Only completed frame leases cross from Core1.
    let panel = unsafe { plane::PanelOwner::new(panel) };
    plane_spawner.spawn(plane::plane_task(panel, plane_display, panel_te).unwrap());
    info!("plane on interrupt executor (FROM_CPU_INTR1, priority 2)");
    match touch {
        Some(touch) => {
            spawner.spawn(input::input_task(gpio6, touch).unwrap());
            info!("R1 hardware tasks spawned: plane, input");
        }
        None => info!("R1 hardware tasks spawned: plane (input task disabled)"),
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
    let camera_executor = alloc::boxed::Box::leak(alloc::boxed::Box::new(InterruptExecutor::new(
        peripherals.FROM_CPU_INTR2,
    )));
    camera_executor
        .start(Priority::Priority3)
        .spawn(crate::platform::drivers::camera::dvp::receive_task().unwrap());
    services.spawn(
        crate::services::camera::task::task(crate::services::camera::task::Hardware(
            peripherals.GPIO48,
            peripherals.GPIO34,
            peripherals.LCD_CAM,
            peripherals.DMA_AXI_CH2,
            peripherals.GPIO16,
            peripherals.GPIO15,
            peripherals.GPIO33,
            peripherals.GPIO18,
            peripherals.GPIO55,
            peripherals.GPIO19,
            peripherals.GPIO17,
        ))
        .unwrap(),
    );

    // Expansion slots: EEPROM address pins (left GPIO14 low -> 0x50, right
    // GPIO39 high -> 0x51) and the Interaction board pins of each side. The
    // pins stay high-impedance until a slot's EEPROM reports an Interaction
    // board (see `crate::platform::drivers::subboard`).
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

    #[cfg(not(feature = "dual-core-ui"))]
    {
        spawner.spawn(ui::ui_task(plane_ui).unwrap());
        spawner.spawn(lua::lua_task().unwrap());
        info!("UI/Lua executor on Core0 (single-core mode)");
    }
    #[cfg(feature = "dual-core-ui")]
    ui_executor::start(peripherals.CPU_CTRL, plane_ui);

    spawner.spawn(crate::services::power::performance::task().unwrap());

    // Everything else runs in the spawned tasks.
    core::future::pending::<()>().await;
}

/// Mount `/system` and `/data` once the console is up (the UI task scans the
/// apps when this is done).
async fn mount_storage(flash: esp_hal::peripherals::FLASH<'static>) {
    embassy_time::Timer::after_millis(1500).await;
    if !crate::services::power::battery_task::wait_boot().await {
        crate::platform::storage::nor::mark_ready();
        return;
    }
    if crashlog::skip(crashlog::Stage::Storage) {
        warn!("fs: not mounted this boot (the last one panicked there); built-in apps only");
    } else {
        crashlog::enter(crashlog::Stage::Storage);
        crate::platform::storage::nor::init(flash);
        crate::platform::storage::nand::mount();
        crate::boot::storage::finish_data_init();
        crashlog::enter(crashlog::Stage::Running);
    }
    crate::platform::storage::nor::mark_ready();
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

use crate::console::transport as console;
use crate::platform::board;
use crate::platform::runtime::crashlog;
use crate::platform::runtime::executors::services as service_executor;
#[cfg(feature = "dual-core-ui")]
use crate::platform::runtime::executors::ui as ui_executor;
use crate::platform::runtime::resident_budget;
use crate::services::update::ota::device as ota_runtime;

pub(crate) mod storage;
