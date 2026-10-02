//! LVGL 9.6 集成层：显示注册、触摸 indev、tick 源与主循环。
//!
//! 数据流：lv_demo_widgets → LVGL(PARTIAL 渲染, RGB888 缓冲) → flush 回调
//! 转 RGB565 写入面板 PSRAM 帧缓冲 → 脏标记 → 主循环 present() 整帧 QSPI 刷屏。

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;

use esp_hal::{
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    i2c::master::I2c,
    time::{Instant, Rate},
};
use log::info;

use lvgl::input_device::{InputDriver, Pointer, PointerInputData};
use lvgl::point::Point;
use lvgl::timer::LvClock;
use lvgl::Display as LvglDisplay;

use crate::{board, drivers::display, drivers::shared_i2c, drivers::touch};

/// LVGL 部分渲染缓冲：1200 像素（480×2.5 行）。注意缓冲按 lv_color_t
/// （RGB888, 4B/px）分配，且 lvgl crate 的 flush trampoline 会把
/// [Color; N] 整个放在栈上——N 必须保持小，否则击穿 MCU 主任务栈
/// （8K 像素 = 32KB 栈数组会直接栈溢出黑屏）。
const DRAW_BUF_PIXELS: usize = 1200;

static FRAME_DIRTY: AtomicBool = AtomicBool::new(false);

/// 单核阻塞式固件：flush 回调仅在主循环的 task_handler 内触发，与
/// present() 严格互斥，因此面板帧缓冲用 static 传递是安全的
/// （与 fan-firmware display.rs 的 STAGING 同一模式）。
static mut PANEL_FB: Option<&'static mut [u16]> = None;

/// LVGL tick 源：开机时刻起的墙钟，经 update_clock 首次调用时注册为
/// lv_tick_set_cb 回调（rust_timer 特性）。
struct BootClock {
    start: Instant,
}

impl LvClock for BootClock {
    fn since_init(&self) -> Duration {
        // esp-hal 的 Instant/Duration 与 core 类型不同，转成 LVGL 需要的 core Duration。
        Duration::from_millis(self.start.elapsed().as_millis() as u64)
    }
}

/// 将帧缓冲整体填充为 RGB565 颜色并立即上屏（启动阶段自检用）。
fn paint_solid(panel: &mut display::Display, px: u16) {
    let fb = unsafe {
        (&raw const PANEL_FB)
            .read()
            .expect("panel framebuffer not installed")
    };
    for p in fb.iter_mut() {
        *p = px;
    }
    let _ = panel.present();
}

pub fn run() -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    // 片内堆（LVGL 对象/部分缓冲）+ PSRAM 后备（面板帧缓冲走 External 显式分配）。
    esp_alloc::heap_allocator!(size: 256 * 1024);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);

    info!(
        "LVGL widgets demo 启动 (ESP-Mosaico / 480x480 QSPI AMOLED) CPU={}MHz",
        esp_hal::clock::cpu_clock().as_mhz()
    );

    // 硬件版本自检：本固件按 v1.2 布线（v1.1 同 v1.2）。
    match board::board_variant() {
        Ok((major, minor)) => info!("板硬件版本 v{major}.{minor}"),
        Err(v) => panic!(
            "不支持的硬件版本 v{}.{}: 无法安全选择引脚映射",
            v.major, v.minor
        ),
    }

    // 上电顺序（官方 BSP 铁律）: VCC_PW(GPIO60, 低有效) 先于一切外设。
    let _vcc_pw = Output::new(p.GPIO60, Level::Low, OutputConfig::default());

    // CO5300 QSPI AMOLED。
    let mut panel = display::init(
        p.SPI2,
        p.DMA_AXI_CH0,
        p.GPIO42, // SCL (v1.2)
        p.GPIO36, // D0
        p.GPIO51, // D1
        p.GPIO35, // D2
        p.GPIO9,  // D3
        p.GPIO50, // CS
        p.GPIO44, // RST (v1.2)
    );

    // 帧缓冲交给 flush 回调（'static，生命周期绑定在 PSRAM 分配上）。
    unsafe {
        PANEL_FB = Some(panel.fb_static());
    }

    // 上电自检（用户可见的启动进度）：白屏 = 面板 + DMA 整帧路径可用。
    paint_solid(&mut panel, 0xFFFF);
    let _ = panel.present();
    info!("自检：白屏已显示（面板初始化完成）");

    // CST9220 触摸: 主板 I2C0 @ SDA=56/SCL=3, 400kHz。
    let i2c = I2c::new(
        p.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("I2C0 初始化失败")
    .with_sda(p.GPIO56)
    .with_scl(p.GPIO3);
    shared_i2c::install(i2c);
    let touch_dev = touch::Cst9220::new();
    if touch_dev.is_some() {
        info!("CST9220 触摸已就绪");
    } else {
        info!("未检测到触摸控制器，demo 将无触摸输入");
    }

    // LVGL 初始化（unsafe_no_autoinit 特性下需显式调用）。
    lvgl::init();

    // 上电自检第二步：绿屏 = lv_init 完成。
    paint_solid(&mut panel, 0x07E0);
    let _ = panel.present();
    info!("自检：绿屏已显示（LVGL 初始化完成）");

    // 显示注册: PARTIAL 渲染 + flush 回调（RGB888 → RGB565 写入面板 fb）。
    let lv_display = LvglDisplay::register::<_, DRAW_BUF_PIXELS>(
        board::LCD_WIDTH as u32,
        board::LCD_HEIGHT as u32,
        |refresh| {
            // SAFETY: 单核阻塞，flush 只在下方 task_handler 内发生。
            let fb = unsafe {
                (&raw const PANEL_FB)
                    .read()
                    .expect("panel framebuffer not installed")
            };
            let area = &refresh.area;
            let mut i = 0usize;
            for y in area.y1..=area.y2 {
                for x in area.x1..=area.x2 {
                    let c = refresh.colors[i];
                    i += 1;
                    let px565 =
                        ((c.r() as u16 >> 3) << 11) | ((c.g() as u16 >> 2) << 5) | (c.b() as u16 >> 3);
                    fb[y as usize * board::LCD_WIDTH as usize + x as usize] = px565;
                }
            }
            FRAME_DIRTY.store(true, Ordering::Release);
        },
    )
    .expect("LVGL display 注册失败");
    // 颜色格式对齐：lvgl crate 的缓冲按 lv_color_t（RGB888, 3B/px）分配、
    // flush trampoline 也按 3B/px 解析，而 LVGL 9 display 默认是 RGB565
    // （2B/px）——不显式设置的话 LVGL 会按 2B/px 计算可渲染像素数，首次
    // 渲染的 flush 区域就会超出 trampoline 的 [Color; N] 数组导致越界 panic。
    unsafe {
        lvgl::sys::lv_display_set_color_format(
            lvgl::sys::lv_display_get_default(),
            lvgl::sys::lv_color_format_t_LV_COLOR_FORMAT_RGB888,
        );
    }
    // 上电自检第三步：蓝屏 = LVGL display 注册成功。
    paint_solid(&mut panel, 0x001F);
    let _ = panel.present();
    info!("自检：蓝屏已显示（display 注册成功）");

    // 触摸 → LVGL pointer indev。无效报告保持上一次状态（不制造虚假抬起）。
    // InputDriver 的回调是 Fn：用 Cell/RefCell 承载轮询状态（单核阻塞，无竞争）。
    let last_down = core::cell::Cell::new(false);
    let last_x = core::cell::Cell::new(0i32);
    let last_y = core::cell::Cell::new(0i32);
    let touch_cell = core::cell::RefCell::new(touch_dev);
    let _pointer = Pointer::register(
        move || {
            if let Some(dev) = touch_cell.borrow_mut().as_mut() {
                match dev.poll() {
                    Ok(touch::TouchState::Down { x, y }) => {
                        last_down.set(true);
                        last_x.set(x as i32);
                        last_y.set(y as i32);
                    }
                    Ok(touch::TouchState::Up) => {
                        last_down.set(false);
                    }
                    Err(_) => {} // I2C/解析错误: 保持上一次状态
                }
            }
            let point = PointerInputData::Touch(Point::new(last_x.get(), last_y.get()));
            if last_down.get() {
                point.pressed().once()
            } else {
                point.released().once()
            }
        },
        &lv_display,
    )
    .expect("pointer indev 注册失败");
    // 上电自检第四步：黄屏 = 触摸 indev 注册成功。
    paint_solid(&mut panel, 0xFFE0);
    let _ = panel.present();
    info!("自检：黄屏已显示（indev 注册成功）");

    // tick 源（首次 update_clock 调用会注册 lv_tick_set_cb）。
    let clock = BootClock {
        start: Instant::now(),
    };

    // 上电自检第五步：紫屏 = 即将测试 Rust 渲染管线。
    paint_solid(&mut panel, 0x780F);
    let _ = panel.present();
    info!("自检：紫屏已显示（进入渲染自检）");

    // 渲染管线自检：纯 Rust API 创建 Label 并渲染几帧（不经过 C demo）。
    // 若崩溃循环停在紫屏/紫屏文字前 → 问题在渲染或 flush 管线；
    // 若文字显示后崩溃 → 渲染正常，问题锁定在 lv_demo_widgets 的 C 代码。
    let mut screen = lv_display.get_scr_act().expect("active screen");
    let mut label = lvgl::widgets::Label::create(&mut screen).expect("create label");
    label.set_text(cstr_core::CString::new("LVGL RENDER OK").unwrap().as_c_str());
    for _ in 0..60 {
        let _ = lvgl::timer::update_clock(&clock);
        lvgl::task_handler();
        if FRAME_DIRTY.swap(false, Ordering::AcqRel) {
            let _ = panel.present();
        }
        esp_hal::delay::Delay::new().delay_millis(5);
    }
    info!("自检：Rust 渲染管线通过");

    // 官方 widgets 全组件示例（vendored 于 lvgl-sys, demo-widgets 特性编译）。
    unsafe {
        lvgl::sys::lv_demo_widgets();
    }
    info!("lv_demo_widgets 已加载，进入主循环");

    loop {
        // LVGL tick: 每轮喂当前时刻（内部 lv_tick_set_cb 已注册）。
        let _ = lvgl::timer::update_clock(&clock);
        lvgl::task_handler();
        if FRAME_DIRTY.swap(false, Ordering::AcqRel) {
            let _ = panel.present();
        }
        esp_hal::delay::Delay::new().delay_millis(5);
    }
}
