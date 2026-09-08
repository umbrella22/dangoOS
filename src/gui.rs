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
use log::{error, info, warn};

use crate::{board, display, AppWindow};

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

/// 创建 AppWindow、注册回调,并进入主渲染循环(带错误恢复)。
pub async fn run_ui(window: Rc<MinimalSoftwareWindow>) -> ! {
    let ui = AppWindow::new().expect("创建 AppWindow 失败");

    // UI → 任务:滑块变化写入共享档位
    ui.on_fan_speed_changed(|speed| {
        crate::fan::set_fan_speed(speed.clamp(0, 100) as u8);
    });

    // UI → 任务:灯开关(STATUS_LED GPIO3,低电平点亮;实际翻转在 main 注册的闭包里)
    ui.on_light_toggled(fire_light_toggled);

    ui.show().expect("显示窗口失败");

    info!("进入 UI 主循环(动态帧率)");

    let mut consecutive_errors = 0;
    const MAX_CONSECUTIVE_ERRORS: u8 = 5;

    loop {
        let frame_start = embassy_time::Instant::now();

        // 驱动 Slint 定时器与动画
        slint::platform::update_timers_and_animations();

        // 渲染并刷新显示(带错误处理)
        let _rendered = window.draw_if_needed(|renderer| {
            renderer.render(display::frame_buffer(), board::LCD_WIDTH as usize);
        });

        // 执行刷新
        let result = display::flush();

        match result {
            Ok(_) => {
                consecutive_errors = 0; // 成功则重置错误计数
            }
            Err(e) => {
                consecutive_errors += 1;
                error!(
                    "显示刷新失败({}/{}): {:?}",
                    consecutive_errors, MAX_CONSECUTIVE_ERRORS, e
                );

                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    warn!("连续失败达阈值,尝试恢复显示驱动");
                    match display::try_recover() {
                        Ok(_) => {
                            info!("显示驱动恢复成功");
                            consecutive_errors = 0;
                        }
                        Err(recover_err) => {
                            error!("显示驱动恢复失败: {:?}", recover_err);
                            // 继续运行但降低帧率以减少 SPI 压力
                            embassy_time::Timer::after(embassy_time::Duration::from_millis(100))
                                .await;
                            continue;
                        }
                    }
                }
            }
        }

        // 动态帧率:补偿渲染耗时以维持目标帧时间
        let elapsed_ms = frame_start.elapsed().as_millis();
        let sleep_ms = board::TARGET_FRAME_TIME_MS.saturating_sub(elapsed_ms);

        if elapsed_ms > board::TARGET_FRAME_TIME_MS {
            warn!("帧时间超标: {}ms > {}ms", elapsed_ms, board::TARGET_FRAME_TIME_MS);
        }

        embassy_time::Timer::after(embassy_time::Duration::from_millis(sleep_ms)).await;
    }
}
