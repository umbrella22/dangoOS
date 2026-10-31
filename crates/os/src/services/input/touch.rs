//! 输入任务：GPIO6 下降沿中断 + 异步 I2C 读取 CST9220。
//!
//! 数据流（`docs/architecture/project-structure.md`）:
//!
//! ```text
//! GPIO6 falling edge ─┐
//!                     ├─> input_task: poll().await -> TouchEvent
//! 5 ms fallback Timer ┘        -> INPUT channel (bounded 32)
//! ```
//!
//! 规则:
//! - 任务只访问触摸控制器和 [`INPUT`]；从不调用 LVGL，也不阻塞在满队列上。
//! - GPIO6 由调用方配置为带上拉的输入（[`touch_irq_input`] 提供该配置）；
//!   任务本身不重配置引脚。中断只唤醒任务，ISR 不做 I2C。
//! - 5 ms 定时器与边沿 future 通过 `select` 竞争，漏掉中断时仍能轮询。
//! - 每次 `poll()` 内的 I2C 事务有 100 ms 超时（`shared_i2c`）。
//!
//! 队列满时丢弃最旧事件，保证投递总是指向最新的
//! 按下/抬起状态，绝不阻塞输入路径。

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use esp_hal::gpio::{Input, InputConfig, InputPin, Pull};
use log::{info, warn};

use {crate::platform::drivers::touch::Cst9220, crate::platform::drivers::touch::TouchState};

/// 队列容量（接口冻结: `Channel<..., TouchEvent, 32>`）。
pub const INPUT_CAPACITY: usize = 32;
/// 漏中断回退轮询周期。
pub const FALLBACK_POLL_PERIOD: Duration = Duration::from_millis(5);

/// 一个触摸采样。`at_us` 是报告读取完成时的单调微秒时间戳。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchEvent {
    pub at_us: u64,
    pub x: u16,
    pub y: u16,
    pub pressed: bool,
}

/// `input_task` -> UI 任务的定长事件队列。
pub static INPUT: Channel<CriticalSectionRawMutex, TouchEvent, INPUT_CAPACITY> = Channel::new();

/// 构造 GPIO6 触摸中断输入: 低有效 + 内部上拉。
///
/// 由 `main` 在 spawn 前调用一次，任务内不再改动引脚配置。
pub fn touch_irq_input(gpio: impl InputPin + 'static) -> Input<'static> {
    Input::new(gpio, InputConfig::default().with_pull(Pull::Up))
}

fn now_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

/// 非阻塞发布: 队列满时丢弃最旧事件再重试。
fn publish(event: TouchEvent) {
    if INPUT.try_send(event).is_err() {
        // TouchEvent 是 Copy；丢弃队首（最旧）事件后重试。
        let _ = INPUT.try_receive();
        let _ = INPUT.try_send(event);
    }
}

/// 等待 GPIO6 下降沿或 5 ms 回退定时器，然后读取一份 CST9220 报告。
///
/// 每次有效报告都会进入 [`INPUT`]，由 UI 侧按顺序处理。
#[embassy_executor::task]
pub async fn input_task(mut gpio6: Input<'static>, mut touch: Cst9220) {
    info!(
        "input_task: GPIO6 falling-edge trigger active, {} ms fallback, queue={}",
        FALLBACK_POLL_PERIOD.as_millis(),
        INPUT_CAPACITY
    );

    let mut pressed = false;
    let mut last_x = 0u16;
    let mut last_y = 0u16;
    let mut errors: u32 = 0;

    loop {
        // 边沿或回退定时器，任一先到都读取一次报告。`wait_for_falling_edge`
        // 的 future 被取消后不再监听；下一次循环会重新监听，期间的遗漏由
        // 5 ms 回退覆盖。
        match select(
            gpio6.wait_for_falling_edge(),
            Timer::after(FALLBACK_POLL_PERIOD),
        )
        .await
        {
            Either::First(()) | Either::Second(()) => {}
        }

        let report = touch.poll().await;
        match report {
            Ok(TouchState::Down { x, y }) => {
                let at_us = now_us();
                crate::services::power::coordinator::wake();
                pressed = true;
                last_x = x;
                last_y = y;
                publish(TouchEvent {
                    at_us,
                    x,
                    y,
                    pressed: true,
                });
            }
            Ok(TouchState::Up) => {
                if pressed {
                    let at_us = now_us();
                    pressed = false;
                    publish(TouchEvent {
                        at_us,
                        x: last_x,
                        y: last_y,
                        pressed: false,
                    });
                }
            }
            Err(error) => {
                // 限速错误日志，避免持续故障占满控制台队列。
                if errors == 0 || errors.is_multiple_of(200) {
                    warn!("input_task: CST9220 poll error #{errors}: {error}");
                }
                errors = errors.saturating_add(1);
            }
        }
    }
}
