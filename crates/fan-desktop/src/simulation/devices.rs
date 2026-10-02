// 模拟硬件实现（用于开发测试）
// 在桌面环境运行，无需真实硬件

use super::state::{CpuTempSensor, FanController, TemperatureSensor};
use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

// ============================================
// 模拟温度传感器
// ============================================

/// 模拟温度传感器
///
/// 返回基础温度 ± 随机波动
pub struct MockTempSensor {
    base_celsius: f32,
}

impl MockTempSensor {
    pub fn new(base_celsius: f32) -> Self {
        Self { base_celsius }
    }
}

impl TemperatureSensor for MockTempSensor {
    fn read_celsius(&self) -> Result<f32, &'static str> {
        // 模拟温度波动 ±0.5°C
        let noise = (rand::random::<u8>() % 10) as f32 / 10.0 - 0.5;
        Ok(self.base_celsius + noise)
    }
}

// ============================================
// 模拟风扇控制器
// ============================================

/// 模拟风扇控制器
///
/// 模拟 PWM 占空比和转速响应
pub struct MockFanController {
    duty: AtomicU8,         // PWM 占空比 (0-100)
    target_rpm: AtomicU16,  // 目标转速
    current_rpm: AtomicU16, // 当前转速
}

impl MockFanController {
    pub fn new() -> Self {
        Self {
            duty: AtomicU8::new(75),
            target_rpm: AtomicU16::new(2400),
            current_rpm: AtomicU16::new(2400),
        }
    }
}

impl FanController for MockFanController {
    fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str> {
        let clamped = percentage.min(100);
        self.duty.store(clamped, Ordering::Relaxed);

        // 计算目标转速：假设 0% = 100 RPM, 100% = 3300 RPM
        let target = 100 + (clamped as u16 * 32);
        self.target_rpm.store(target, Ordering::Relaxed);

        Ok(())
    }

    fn read_rpm(&self) -> Result<u16, &'static str> {
        // 模拟转速平滑过渡
        let current = self.current_rpm.load(Ordering::Relaxed);
        let target = self.target_rpm.load(Ordering::Relaxed);

        let new_rpm = if current < target {
            (current + 50).min(target)
        } else if current > target {
            current.saturating_sub(50).max(target)
        } else {
            current
        };

        self.current_rpm.store(new_rpm, Ordering::Relaxed);
        Ok(new_rpm)
    }
}

// ============================================
// 模拟 CPU 温度传感器
// ============================================

/// 模拟 CPU 温度传感器
///
/// 返回 38-48°C 之间的随机温度
pub struct MockCpuTempSensor;

impl CpuTempSensor for MockCpuTempSensor {
    fn read_celsius(&self) -> Result<i32, &'static str> {
        // 模拟 CPU 温度在 38-48°C 之间波动
        Ok(38 + (rand::random::<u8>() % 11) as i32)
    }
}
