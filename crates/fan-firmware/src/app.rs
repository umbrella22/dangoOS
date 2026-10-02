//! ikaros 风扇控制 - ESP-Mosaico (v1.2) 真机固件入口
//!
//! 架构（阻塞式，无 RTOS）：
//!   CO5300 QSPI AMOLED 480×480 (SPI2 + AXI GDMA, GRAM 自扫描)
//!   CST9220 触摸 (I2C0 @ SDA=56/SCL=3, 400kHz) → WindowEvent
//!   GPIO12 风扇 PWM 默认禁用；fan-pwm-gpio12 显式保留后供驱动 API 使用
//!
//! 引脚/上电顺序以官方 BSP v1.2 映射为准，
//! 见 `board.rs` 与 `docs/MOSAICO_V12_HARDWARE.md`。
//!
//! 网络/电量/温度仍用 UI 占位；IMU 样本来自已初始化的板载 BMI270。

use crate::{
    drivers::{camera, display, fan, imu, shared_i2c, touch},
    platform_mosaic,
};

use esp_hal::{
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Output, OutputConfig},
    i2c::master::I2c,
    time::Rate,
};
use log::info;

pub fn run() -> ! {
    // 串口日志（UART0, 115200）—— 真机调试生命线
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    // esp-alloc 按注册顺序选择堆：UI 对象、属性和绘制临时数据优先使用片内 SRAM。
    // 256 KiB 后仍为主栈/中断留出约 104 KiB；片内不足时普通分配可回退到 PSRAM。
    // 显示驱动通过 External capability 显式分配整块帧缓冲。
    esp_alloc::heap_allocator!(size: 256 * 1024);

    // PSRAM 保存帧缓冲，并为全局堆提供后备空间（250MHz 经 Korvo 真机验证的预设）。
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);

    info!(
        "ikaros 风扇控制固件启动 (ESP-Mosaico / 480x480 QSPI AMOLED) CPU={}MHz",
        esp_hal::clock::cpu_clock().as_mhz()
    );

    // 硬件版本自检: 本固件的引脚按 v1.2 布线（v1.1 同 v1.2）。
    // 版本号在 eFuse USER_DATA 前 16 位，官方 BSP 同款读取。
    match crate::board::board_variant() {
        Ok(v) => {
            let ver = crate::board::hardware_version();
            info!(
                "板硬件版本 v{}.{} (variant={})",
                ver.major,
                ver.minor,
                if matches!(v, crate::board::BoardVariant::V1_2) {
                    "v1.2"
                } else {
                    "v1.0"
                }
            );
            if matches!(v, crate::board::BoardVariant::V1_0) {
                panic!("检测到 v1.0 硬件: 本固件按 v1.2 引脚编译, 拒绝在 v1.0 上运行");
            }
        }
        Err(ver) => panic!(
            "不支持的硬件版本 v{}.{} (raw): 无法安全选择引脚映射",
            ver.major, ver.minor
        ),
    }

    // 上电顺序（官方 BSP 铁律）: VCC_PW(GPIO60, 低有效) 必须先于一切外设
    let _vcc_pw = Output::new(p.GPIO60, Level::Low, OutputConfig::default());
    // PWR_SW(GPIO57) 为关机信号, 开漏释放=高阻; 保持浮空输入即等效释放

    // CO5300 QSPI: RST 时序 + 厂商初始化序列在驱动内完成
    let mut display = display::init(
        p.SPI2,
        p.DMA_AXI_CH0,
        p.GPIO42, // SCL (v1.2; v1.0 为 44, 与 RST 互换)
        p.GPIO36, // D0
        p.GPIO51, // D1
        p.GPIO35, // D2
        p.GPIO9,  // D3
        p.GPIO50, // CS
        p.GPIO44, // RST (v1.2)
    );
    info!("AMOLED 显示已启动 (40MHz QSPI / GRAM 自扫描)");

    // CST9220: 主板 I2C0 在 v1.2 上为 SDA=56/SCL=3, 400kHz
    let i2c = I2c::new(
        p.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("I2C0 初始化失败")
    .with_sda(p.GPIO56)
    .with_scl(p.GPIO3);
    shared_i2c::install(i2c);
    let mut touch = touch::Cst9220::new();
    if touch.is_some() {
        info!("CST9220 触摸已就绪 (I2C=400kHz, 官方 v0.1.1 协议)");
    } else {
        info!("未检测到触摸控制器，UI 将无触摸输入");
    }

    // 风扇 PWM: 模块座 GPIO12 (RMT 连续波形 25kHz)
    // ⚠️ 左槽 H8 与模块复用, 插左槽模块前先仲裁（见 board::subboard 注释）
    #[cfg(feature = "fan-pwm-gpio12")]
    let mut fan = Some(fan::FanPwm::new(p.GPIO12, p.RMT));
    #[cfg(not(feature = "fan-pwm-gpio12"))]
    let mut fan: Option<fan::FanPwm> = None;
    #[cfg(not(feature = "fan-pwm-gpio12"))]
    info!("GPIO12 风扇 PWM 已禁用；扩展槽引脚需显式独占后启用");

    // Camera uses its own bounded I2C1; never changes the touch I2C0 bus.
    #[cfg(not(feature = "fan-pwm-gpio12"))]
    let mut camera = {
        use esp_hal::gpio::Pin;
        let bus = I2c::new(
            p.I2C1,
            esp_hal::i2c::master::Config::default()
                .with_frequency(Rate::from_khz(100))
                .with_software_timeout(esp_hal::i2c::master::SoftwareTimeout::Transaction(
                    esp_hal::time::Duration::from_millis(100),
                )),
        )
        .expect("相机 I2C1 初始化失败")
        .with_sda(p.GPIO0)
        .with_scl(p.GPIO1);
        camera::CameraModule::new(
            bus,
            camera::Hardware {
                lcd_cam: p.LCD_CAM,
                dma: p.DMA_AXI_CH1,
                data: [
                    p.GPIO16.degrade(),
                    p.GPIO15.degrade(),
                    p.GPIO33.degrade(),
                    p.GPIO4.degrade(),
                    p.GPIO14.degrade(),
                    p.GPIO12.degrade(),
                    p.GPIO18.degrade(),
                    p.GPIO13.degrade(),
                ],
                pclk: p.GPIO17.degrade(),
                vsync: p.GPIO55.degrade(),
                href: p.GPIO19.degrade(),
                reset: p.GPIO53.degrade(),
                pwdn: p.GPIO48.degrade(),
                flash: p.GPIO34.degrade(),
            },
        )
    };
    #[cfg(feature = "fan-pwm-gpio12")]
    let mut camera = camera::CameraModule::conflict();

    // AI 键(GPIO7, 低有效): 锁屏/解锁
    let mut ai_key = Input::new(
        p.GPIO7,
        InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
    );

    // BMI270 IMU(主板 I2C0 @0x69, 与触摸共享总线)
    let imu = imu::Bmi270::new();

    #[cfg(feature = "lcd-color-test")]
    show_color_test(&mut display, &mut touch, &mut fan);

    // 进入官方风格 Hub UI 主循环（永不返回）
    platform_mosaic::run(
        &mut display,
        &mut touch,
        &mut fan,
        &mut ai_key,
        imu,
        &mut camera,
    );
}

/// Raw RGB565 output uses the normal DMA path, with no fonts or alpha blending.
#[cfg(feature = "lcd-color-test")]
fn show_color_test(
    display: &mut display::Display,
    touch: &mut Option<touch::Cst9220>,
    fan: &mut Option<fan::FanPwm>,
) {
    use esp_hal::{delay::Delay, time::Instant};
    use slint::platform::software_renderer::Rgb565Pixel;

    let buffer = display.fb_mut();
    const COLORS: [u16; 8] = [
        0x0000, 0xffff, 0xf800, 0x07e0, 0x001f, 0x07ff, 0xf81f, 0xffe0,
    ];
    let (w, h) = (crate::board::LCD_WIDTH, crate::board::LCD_HEIGHT);
    for y in 0..h {
        for x in 0..w {
            let band = y < h / 6;
            let pixel = if band {
                COLORS[x as usize / (w as usize / 8)]
            } else if (x / 8 + y / 8) % 2 == 0 {
                0xffff
            } else {
                0
            };
            buffer[(y as usize) * (w as usize) + (x as usize)] = Rgb565Pixel(pixel);
        }
    }
    let _ = display.present();
    let delay = Delay::new();
    loop {
        match display.poll_present() {
            Some(true) => break,
            Some(false) => {
                log::warn!("LCD 彩条发布超时");
                return;
            }
            None => delay.delay_millis(1),
        }
    }
    info!("LCD 原始 RGB565 测试 15s：彩条/棋盘；随后进入 UI");
    let started = Instant::now();
    while started.elapsed().as_millis() < 15_000 {
        if let Some(tp) = touch.as_mut() {
            let _ = tp.poll();
        }
        if let Some(fan) = fan.as_mut() {
            fan.tick();
        }
        delay.delay_millis(8);
    }
}
