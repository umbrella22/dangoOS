//! Slint ↔ esp-rtos 集成层。
//!
//! - `EspPlatform`:把 Slint 的时钟挂到 esp-rtos 上(动画计时依赖它);
//! - `init_platform` / `run_ui`:创建软件渲染窗口并驱动主渲染循环。
//!
//! 显示:CO5300 AMOLED 有 GRAM 自刷新,渲染循环把整帧写进帧缓冲后
//! 经 QSPI 推给面板(`display::flush()`)。

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::time::Duration;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::ComponentHandle;

use crate::{display, AppWindow};

/// 灯开关回调(由 main 注册:翻转 GPIO3 状态灯)
static mut LIGHT_TOGGLED: Option<Box<dyn Fn()>> = None;

pub fn on_light_toggled(f: impl Fn() + 'static) {
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(LIGHT_TOGGLED);
        *slot = Some(Box::new(f));
    }
}

fn fire_light_toggled() {
    unsafe {
        if let Some(f) = &*core::ptr::addr_of_mut!(LIGHT_TOGGLED) {
            f();
        }
    }
}

struct EspPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: embassy_time::Instant,
}

impl Platform for EspPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_micros(self.start.elapsed().as_micros() as u64)
    }
}

/// 初始化 Slint 平台层,返回全局唯一的软件渲染窗口。
/// 只能调用一次,且必须先于任何 UI 组件创建。
pub fn init_platform() -> Rc<MinimalSoftwareWindow> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::SwappedBuffers);
    slint::platform::set_platform(Box::new(EspPlatform {
        window: window.clone(),
        start: embassy_time::Instant::now(),
    }))
    .expect("Slint Platform 只能初始化一次");
    window
}

/// 创建 AppWindow、注册回调,并进入主渲染循环。
pub async fn run_ui(window: Rc<MinimalSoftwareWindow>) -> ! {
    let ui = AppWindow::new().expect("创建 AppWindow 失败");

    // UI → 任务:滑块变化写入共享档位
    ui.on_fan_speed_changed(|speed| {
        crate::fan::set_fan_speed(speed.clamp(0, 100) as u8);
    });

    // UI → 任务:灯开关(STATUS_LED GPIO3,低电平点亮;实际翻转在 main 注册的闭包里)
    ui.on_light_toggled(fire_light_toggled);

    ui.show().expect("显示窗口失败");

    loop {
        // 驱动 Slint 定时器与动画
        slint::platform::update_timers_and_animations();

        window.draw_if_needed(|renderer| {
            // 渲染进帧缓冲后,整帧经 QSPI 写入 AMOLED GRAM
            renderer.render(display::frame_buffer(), crate::board::LCD_WIDTH as usize);
            display::flush();
        });

        // ~60fps 的节拍;后续可改为触摸/TE 防撕裂驱动
        embassy_time::Timer::after(embassy_time::Duration::from_millis(16)).await;
    }
}
