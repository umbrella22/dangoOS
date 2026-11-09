//! Core0-only assembly. USB starts before panel, touch, NAND or filesystem work.
mod install;
mod screen;
mod staged;
mod storage;
mod usb;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use embassy_executor::Spawner;
use embassy_time::Timer;
use esp_hal::{
    gpio::{DriveMode, Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::I2c,
    interrupt::Priority,
    time::Rate,
    timer::timg::TimerGroup,
};

pub(super) static DISPLAY: AtomicU8 = AtomicU8::new(0);
pub(super) static TOUCH: AtomicU8 = AtomicU8::new(0);
pub(super) static TOUCH_EVENTS: AtomicU32 = AtomicU32::new(0);
pub(super) static CANCELLED: AtomicBool = AtomicBool::new(false);
pub(super) static CONNECTED: AtomicBool = AtomicBool::new(false);
pub(super) static ACTIVE: AtomicU8 = AtomicU8::new(0);
pub(super) static POSITION: AtomicU32 = AtomicU32::new(0);
pub(super) static WRITE_MODE: AtomicBool = AtomicBool::new(false);
pub(super) static INTRO_FRAMES: AtomicU32 = AtomicU32::new(0);
pub(super) static INTRO_MS: AtomicU32 = AtomicU32::new(0);
pub(super) static INTRO_DECODE_US: AtomicU32 = AtomicU32::new(0);
pub(super) static INTRO_DRAW_US: AtomicU32 = AtomicU32::new(0);
pub(super) static NAND_READY: AtomicBool = AtomicBool::new(false);
pub(super) static INSTALL_PHASE: AtomicU8 = AtomicU8::new(0);
pub(super) static INSTALL_POSITION: AtomicU32 = AtomicU32::new(0);
pub(super) static INSTALL_TOTAL: AtomicU32 = AtomicU32::new(0);
// Capture the previous main's reset and panic record before HAL initialization.
pub(super) static BOOT_RESET: AtomicU32 = AtomicU32::new(0);
static mut MAIN_PANIC: [u32; 66] = [0; 66];
pub(super) fn boot_diagnostic() -> serde_json::Value {
    // Captured once before starting any executor and only read afterwards.
    let words = unsafe { core::ptr::addr_of!(MAIN_PANIC).read() };
    let raw = words.map(u32::to_le_bytes).concat();
    let text = if words[0] == 0x4d535031 {
        core::str::from_utf8(&raw[16..16 + (words[1] as usize).min(248)]).unwrap_or("invalid UTF-8")
    } else {
        "no panic record"
    };
    serde_json::json!({"reset":BOOT_RESET.load(Ordering::Acquire),"panic_magic":words[0],"panic":text})
}

pub async fn run(spawner: Spawner) {
    BOOT_RESET.store(
        esp_hal::system::reset_reason().map_or(0, |r| r as u32),
        Ordering::Release,
    );
    for i in 0..66 {
        unsafe {
            core::ptr::addr_of_mut!(MAIN_PANIC)
                .cast::<u32>()
                .add(i)
                .write((0x2e000000 as *const u32).add(i).read_volatile());
        }
    }
    let p =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
    esp_alloc::heap_allocator!(size:160*1024);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size:64*1024);
    let _rail = Output::new(p.GPIO60, Level::Low, OutputConfig::default());
    let mut latch = Output::new(
        p.GPIO57,
        Level::High,
        OutputConfig::default().with_drive_mode(DriveMode::OpenDrain),
    );
    latch.set_pad_hold(false);
    let timer = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timer.timer0);

    let executor = Box::leak(Box::new(esp_rtos::embassy::InterruptExecutor::<1>::new(
        p.FROM_CPU_INTR1,
    )));
    executor
        .start(Priority::Priority2)
        .spawn(usb::run(p.USB_HS).unwrap());
    spawner.spawn(usb::media().unwrap());
    let input = InputConfig::default().with_pull(Pull::Up);
    spawner.spawn(
        power(
            Input::new(p.GPIO7, input),
            Input::new(p.GPIO61, input),
            latch,
        )
        .unwrap(),
    );
    // Allow the USB runner and command reader to reach their first poll.
    Timer::after_millis(10).await;
    if board_s31::board::board_variant().is_err() {
        loop {
            Timer::after_millis(1000).await;
        }
    }
    board_s31::nand::init(
        p.SPI3,
        p.DMA_AXI_CH1,
        p.GPIO20,
        p.GPIO21,
        p.GPIO22,
        p.GPIO23,
        p.GPIO24,
        p.GPIO25,
    );
    NAND_READY.store(true, Ordering::Release);
    if let Ok(bus) = I2c::new(
        p.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    ) {
        board_s31::shared_i2c::install_async(bus.with_sda(p.GPIO56).with_scl(p.GPIO3).into_async());
        spawner.spawn(touch().unwrap());
    } else {
        TOUCH.store(2, Ordering::Release);
    }
    let panel = board_s31::display::try_init(
        p.SPI2,
        p.DMA_AXI_CH0,
        p.GPIO42,
        p.GPIO36,
        p.GPIO51,
        p.GPIO35,
        p.GPIO9,
        p.GPIO50,
        p.GPIO44,
    );
    match panel {
        Ok(panel) => screen::run(panel.into_async()).await,
        Err(_) => {
            DISPLAY.store(2, Ordering::Release);
            loop {
                Timer::after_millis(1000).await;
            }
        }
    }
}
#[embassy_executor::task]
async fn touch() {
    if let Some(mut input) = board_s31::touch::Cst9220::new().await {
        TOUCH.store(1, Ordering::Release);
        let mut down = false;
        loop {
            if let Ok(Some(state)) = input.poll().await {
                match state {
                    board_s31::touch::TouchState::Down { x: _, y } => {
                        if !down {
                            TOUCH_EVENTS.fetch_add(1, Ordering::Relaxed);
                            if y >= 430 {
                                CANCELLED.store(true, Ordering::Release);
                            }
                        }
                        down = true;
                    }
                    board_s31::touch::TouchState::Up => down = false,
                }
            }
            Timer::after_millis(20).await;
        }
    } else {
        TOUCH.store(2, Ordering::Release);
    }
}
#[embassy_executor::task]
async fn power(ai: Input<'static>, boot: Input<'static>, mut latch: Output<'static>) {
    let mut since = None;
    let mut armed = false;
    loop {
        if boot.is_low() {
            CANCELLED.store(true, Ordering::Release);
        }
        if ai.is_high() {
            armed = true;
        }
        if armed && ai.is_low() {
            let start = since.get_or_insert(embassy_time::Instant::now());
            if start.elapsed().as_millis() >= 1500 {
                CANCELLED.store(true, Ordering::Release);
                latch.set_low();
                loop {
                    Timer::after_millis(1000).await;
                }
            }
        } else {
            since = None;
        }
        Timer::after_millis(50).await;
    }
}
