//! 桌面模拟器的设备接口和应用状态。

use fan_core::fan::{automatic_duty, estimated_power};
use std::time::Instant;

// ============================================
// 硬件接口 Trait 定义
// ============================================

/// 温度传感器接口
///
/// 实现此 trait 以支持不同的温度传感器
/// 例如模拟设备或主机连接的外部传感器。
pub trait TemperatureSensor: Send {
    /// 读取温度（摄氏度）
    fn read_celsius(&self) -> Result<f32, &'static str>;
}

/// PWM 风扇控制接口
///
/// 实现此 trait 以控制 PWM 风扇
/// 包括: 设置占空比、读取转速反馈
pub trait FanController: Send {
    /// 设置 PWM 占空比 (0-100%)
    fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str>;

    /// 读取风扇转速 (RPM)
    /// 需要风扇有 TACH 引脚反馈
    fn read_rpm(&self) -> Result<u16, &'static str>;
}

/// CPU 温度传感器接口
///
/// 读取 MCU 内部温度传感器
pub trait CpuTempSensor: Send {
    /// 读取 CPU 温度（摄氏度）
    fn read_celsius(&self) -> Result<i32, &'static str>;
}

// ============================================
// 应用状态管理
// ============================================

/// 风扇控制系统状态
pub struct FanControlState {
    // 传感器数据
    pub temperature: f32,
    pub temp_history: Vec<f32>, // 用于计算趋势

    // 风扇数据
    pub fan_rpm: u16,
    pub fan_duty: u8,
    auto_mode: bool,

    // 系统状态
    pub cpu_temp: i32,
    pub uptime_secs: u64,

    // 硬件接口（trait objects）
    temp_sensor: Box<dyn TemperatureSensor>,
    fan_controller: Box<dyn FanController>,
    cpu_sensor: Box<dyn CpuTempSensor>,

    // 计时器
    start_time: Instant,
}

impl FanControlState {
    /// 创建新的状态管理器（模拟硬件）
    pub fn new_mock() -> Self {
        use super::devices::*;

        Self {
            temperature: 28.5,
            temp_history: vec![28.5; 10],
            fan_rpm: 2450,
            fan_duty: 75,
            auto_mode: false,
            cpu_temp: 42,
            uptime_secs: 0,
            temp_sensor: Box::new(MockTempSensor::new(28.5)),
            fan_controller: Box::new(MockFanController::new()),
            cpu_sensor: Box::new(MockCpuTempSensor),
            start_time: Instant::now(),
        }
    }

    /// 使用指定设备创建状态管理器。
    ///
    /// # 参数
    /// - `temp_sensor`: 温度传感器实现
    /// - `fan_controller`: 风扇控制器实现
    /// - `cpu_sensor`: CPU 温度传感器实现
    #[allow(dead_code)] // 保留桌面注入设备的入口
    pub fn new_hardware(
        temp_sensor: Box<dyn TemperatureSensor>,
        fan_controller: Box<dyn FanController>,
        cpu_sensor: Box<dyn CpuTempSensor>,
    ) -> Self {
        Self {
            temperature: 25.0,
            temp_history: vec![25.0; 10],
            fan_rpm: 0,
            fan_duty: 0,
            auto_mode: false,
            cpu_temp: 0,
            uptime_secs: 0,
            temp_sensor,
            fan_controller,
            cpu_sensor,
            start_time: Instant::now(),
        }
    }

    /// 更新所有传感器数据
    pub fn update(&mut self) -> Result<(), &'static str> {
        let now = Instant::now();

        // 更新运行时间
        self.uptime_secs = now.duration_since(self.start_time).as_secs();

        // 读取温度传感器
        match self.temp_sensor.read_celsius() {
            Ok(temp) => {
                self.temperature = temp;
                // 保留最近 60 个采样（1s/次）: 供趋势计算与宽屏趋势图使用
                self.temp_history.push(temp);
                if self.temp_history.len() > 60 {
                    self.temp_history.remove(0);
                }
                if self.auto_mode {
                    let duty = automatic_duty(temp);
                    if duty != self.fan_duty {
                        self.fan_controller.set_duty_cycle(duty)?;
                        self.fan_duty = duty;
                    }
                }
            }
            Err(e) => eprintln!("⚠️ 温度读取失败: {}", e),
        }

        // 读取风扇转速
        match self.fan_controller.read_rpm() {
            Ok(rpm) => self.fan_rpm = rpm,
            Err(e) => eprintln!("⚠️ 转速读取失败: {}", e),
        }

        // 读取 CPU 温度
        match self.cpu_sensor.read_celsius() {
            Ok(temp) => self.cpu_temp = temp,
            Err(e) => eprintln!("⚠️ CPU 温度读取失败: {}", e),
        }

        Ok(())
    }

    /// 设置风扇档位
    ///
    /// # 参数
    /// - `percentage`: 档位百分比，-1 表示 AUTO 模式
    pub fn set_fan_speed(&mut self, percentage: i32) -> Result<(), &'static str> {
        let auto_mode = percentage < 0;
        let duty = if auto_mode {
            // AUTO 模式：根据温度自动调节
            automatic_duty(self.temperature)
        } else {
            percentage.min(100) as u8
        };

        self.fan_controller.set_duty_cycle(duty)?;
        self.fan_duty = duty;
        self.auto_mode = auto_mode;
        Ok(())
    }

    /// 计算温度趋势 (°C/分钟)
    pub fn get_temp_trend(&self) -> f32 {
        if self.temp_history.len() < 2 {
            return 0.0;
        }

        let first = self.temp_history[0];
        let last = self.temp_history[self.temp_history.len() - 1];
        let delta = last - first;

        // 采样间隔 1s，按实际跨度换算为每分钟变化率
        let span_secs = (self.temp_history.len() - 1) as f32;
        delta * 60.0 / span_secs
    }

    /// 计算功率估算 (W)
    ///
    /// 假设风扇最大功耗 8W
    pub fn get_power_estimate(&self) -> f32 {
        estimated_power(self.fan_duty)
    }

    /// 格式化运行时间为 HH:MM:SS
    pub fn format_uptime(&self) -> String {
        let hours = self.uptime_secs / 3600;
        let minutes = (self.uptime_secs % 3600) / 60;
        let seconds = self.uptime_secs % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    }
}
