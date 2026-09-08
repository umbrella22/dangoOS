//! 风扇控制任务:跟随 UI 下发的目标档位。
//! PWM(LEDC)输出尚未接入,当前只做档位跟踪与日志。

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_time::Timer;
use log::info;

/// 风扇目标档位(0~100):UI 滑块回调写入,风扇任务周期读取。
pub static FAN_SPEED_CMD: AtomicU8 = AtomicU8::new(0);

pub fn set_fan_speed(speed: u8) {
    FAN_SPEED_CMD.store(speed, Ordering::Relaxed);
}

#[embassy_executor::task]
pub async fn fan_control_task() {
    info!("风扇控制任务启动");
    let mut applied: Option<u8> = None;

    loop {
        let target = FAN_SPEED_CMD.load(Ordering::Relaxed);
        if Some(target) != applied {
            applied = Some(target);
            // TODO: 接入 esp_hal::ledc PWM,按档位换算占空比,例如:
            //   let duty = (target as u32) * pwm.max_duty() / 100;
            //   channel.set_duty(duty);
            // PWM 引脚候选:board::FAN_PWM_GPIO(GPIO12,模块扩展座 H2)
            info!("风扇档位更新: {}%", target);
        }
        Timer::after_millis(100).await;
    }
}
