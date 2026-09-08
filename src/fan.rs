//! 风扇控制任务:跟随 UI 下发的目标档位。
//! 使用 embassy_sync::signal 实现事件驱动,避免空轮询。
//!
//! 注意:ESP32-S31 不支持 LEDC,需使用 MCPWM 或 GPIO 软件 PWM。
//! 当前实现为占位符,待硬件到手后接入 MCPWM 或其他 PWM 方案。

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use log::info;

use crate::board;

/// 风扇档位变化信号(0~100):UI 滑块回调触发,风扇任务等待
static FAN_SPEED_SIGNAL: Signal<CriticalSectionRawMutex, u8> = Signal::new();

/// 设置风扇目标档位(由 UI 回调调用)
pub fn set_fan_speed(speed: u8) {
    FAN_SPEED_SIGNAL.signal(speed.clamp(0, board::FAN_PWM_MAX_DUTY));
}

/// 风扇控制任务:等待档位信号并更新 PWM 占空比(事件驱动)
///
/// TODO: ESP32-S31 PWM 实现选项:
/// 1. MCPWM (Motor Control PWM) - 如果芯片支持
/// 2. GPIO + Timer 软件 PWM
/// 3. 使用 RMT (Remote Control) 模块生成 PWM
#[embassy_executor::task]
pub async fn fan_control_task() {
    info!("风扇控制任务启动(事件驱动,PWM 待实现)");

    loop {
        // 阻塞等待新档位(无空轮询)
        let speed = FAN_SPEED_SIGNAL.wait().await;

        // TODO: 接入 PWM 输出
        // 方案 1 (MCPWM,如果支持):
        //   use esp_hal::mcpwm::McPwm;
        //   let mut mcpwm = McPwm::new(peripherals.MCPWM0);
        //   mcpwm.operator0.set_duty_a(speed);
        //
        // 方案 2 (软件 PWM):
        //   let duty_us = (speed as u64) * 1000 / 100; // 1kHz PWM
        //   loop {
        //       pin.set_high();
        //       Timer::after_micros(duty_us).await;
        //       pin.set_low();
        //       Timer::after_micros(1000 - duty_us).await;
        //   }
        //
        // PWM 引脚候选:board::FAN_PWM_GPIO(GPIO12,模块扩展座 H2)

        info!("风扇档位更新: {}% (PWM 输出待硬件实现)", speed);
    }
}
