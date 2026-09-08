#![no_std]
#![no_main]
#![allow(static_mut_refs)]

extern crate alloc;

mod board;
mod display;
mod fan;
mod gui;

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_executor::Spawner;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::init;
use esp_hal::spi::{master::{Config as SpiConfig, Spi}, Mode};
use esp_hal::time::Rate;
use esp_hal::timer::timg::{TimerGroup, Wdt};
use log::info;

// build.rs 编译 ui/appwindow.slint 后,在此生成 AppWindow 等类型
slint::include_modules!();

// 显式引入:确保 esp-backtrace 的 #[panic_handler] 被纳入编译图
extern crate esp_backtrace;

// ESP-IDF 应用描述符(espflash 4.x 烧录时校验)
esp_bootloader_esp_idf::esp_app_desc!();

// 状态灯(低电平点亮) - 使用 Mutex + RefCell 代替 static mut
static STATUS_LED: Mutex<RefCell<Option<Output<'static>>>> = Mutex::new(RefCell::new(None));

// 看门狗(喂狗保护) - 使用 Mutex + RefCell
static WATCHDOG: Mutex<RefCell<Option<Wdt<esp_hal::peripherals::TIMG0>>>> =
    Mutex::new(RefCell::new(None));

/// 喂狗函数(由主循环定期调用)
pub fn feed_watchdog() {
    critical_section::with(|cs| {
        if let Some(wdt) = WATCHDOG.borrow_ref_mut(cs).as_mut() {
            wdt.feed();
        }
    });
}

// #[esp_rtos::main] 把 async main 包装成 esp-rtos 线程模式执行器上的任务;
// 调度器在本任务内调用 esp_rtos::start 后才真正运转
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // 1. 堆分配器必须最先初始化:Slint 在 no_std 下依赖动态内存
    esp_alloc::heap_allocator!(size: 72 * 1024);

    // 2. 初始化外设,拉满主频(双核 320MHz)
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = init(config);

    esp_println::logger::init_logger_from_env();
    info!("my-slint-fan 启动(ESP-Mosaico / ESP32-S31)");

    // 3. PSRAM 堆:封装内 16MB,帧缓冲(450KB)从这里分配
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // 4. 看门狗初始化(TIMG0 看门狗,3 秒超时)
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let mut wdt = timg0.wdt;
    wdt.enable();
    wdt.set_timeout(
        esp_hal::timer::timg::MwdtStage::Stage0,
        esp_hal::time::Duration::from_millis(board::WATCHDOG_TIMEOUT_MS),
    );
    info!("看门狗已启用(超时 {}ms)", board::WATCHDOG_TIMEOUT_MS);

    critical_section::with(|cs| {
        *WATCHDOG.borrow_ref_mut(cs) = Some(wdt);
    });

    // 5. 状态灯与 3.3V 电源控制
    let led = Output::new(
        peripherals.GPIO3,
        Level::High, // 高 = 灭
        OutputConfig::default(),
    );
    critical_section::with(|cs| {
        *STATUS_LED.borrow_ref_mut(cs) = Some(led);
    });

    // VCC_3V3_CTRL:拉高开启外设电源轨
    let _vcc_ctrl = Output::new(
        peripherals.GPIO60,
        Level::High,
        OutputConfig::default(),
    );

    // 6. 启动 esp-rtos 调度器:TIMG1 定时器做时间源(TIMG0 给看门狗了)
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    esp_rtos::start(timg1.timer0, peripherals.FROM_CPU_INTR0);

    // 7. 初始化 QSPI(CO5300 480×480 AMOLED):SCLK/D0-D3,引脚见 board.rs
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(board::LCD_SPI_HZ))
            .with_mode(Mode::_0),
    )
    .expect("QSPI 配置错误")
    .with_sck(peripherals.GPIO44)
    .with_sio0(peripherals.GPIO36) // D0
    .with_sio1(peripherals.GPIO51) // D1
    .with_sio2(peripherals.GPIO35) // D2
    .with_sio3(peripherals.GPIO9); // D3

    let cs = Output::new(peripherals.GPIO50, Level::High, OutputConfig::default());
    let rst = Output::new(peripherals.GPIO42, Level::High, OutputConfig::default());
    display::attach(spi, cs, rst);

    // 帧缓冲(PSRAM,450KB)+ CO5300 初始化
    display::alloc_frame_buffer_bytes();
    display::init().expect("显示初始化失败");

    // 8. 初始化 Slint 平台层(必须先于任何 UI 组件创建)
    let window = gui::init_platform();

    // 9. UI 回调:状态灯镜像
    gui::on_light_toggled(|| {
        critical_section::with(|cs| {
            if let Some(led) = STATUS_LED.borrow_ref_mut(cs).as_mut() {
                led.toggle();
            }
        });
    });

    // 10. 业务任务:风扇控制(事件驱动,PWM 待硬件实现)
    let _ = spawner.spawn(fan::fan_control_task().unwrap());

    // 11. 主循环驱动 UI 渲染(unsafe-single-threaded:所有 Slint 调用在同一执行线程)
    // 注意:gui::run_ui 内部会定期需要喂狗,这里启动看门狗喂养任务
    let _ = spawner.spawn(watchdog_task().unwrap());

    gui::run_ui(window).await
}

/// 看门狗喂养任务(独立任务定期喂狗,避免主循环阻塞导致复位)
#[embassy_executor::task]
async fn watchdog_task() {
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_millis(
            board::WATCHDOG_TIMEOUT_MS / 3, // 在超时时间的 1/3 处喂狗
        ))
        .await;
        feed_watchdog();
    }
}
