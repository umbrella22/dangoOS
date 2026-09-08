// EVE 风格风扇控制系统 - Rust 后端
// 硬件接口预留 + 模拟数据（待真机测试）
// 作者: Claude + Kiro
// 日期: 2024

use slint::*;
use std::sync::{Arc, atomic::{AtomicU8, AtomicU16, AtomicBool, Ordering}};
use std::time::{Duration, Instant};

// ============================================
// 硬件抽象层（HAL）
// ============================================

/// 温度传感器接口
/// TODO: 真机实现时替换为实际的 I2C/SPI 传感器驱动
pub trait TemperatureSensor {
    fn read_celsius(&self) -> Result<f32, &'static str>;
}

/// PWM 风扇控制接口
/// TODO: 真机实现时替换为 ESP32 LEDC PWM 驱动
pub trait FanController {
    fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str>;
    fn read_rpm(&self) -> Result<u16, &'static str>;
}

/// CPU 温度读取接口
/// TODO: 真机实现时从 ESP32 内部温度传感器读取
pub trait CpuTempSensor {
    fn read_celsius(&self) -> Result<i32, &'static str>;
}

// ============================================
// 模拟硬件实现（用于开发测试）
// ============================================

/// 模拟温度传感器
pub struct MockTempSensor {
    base_temp: AtomicU8,  // 基础温度 × 10
}

impl MockTempSensor {
    pub fn new(base_celsius: f32) -> Self {
        Self {
            base_temp: AtomicU8::new((base_celsius * 10.0) as u8),
        }
    }
}

impl TemperatureSensor for MockTempSensor {
    fn read_celsius(&self) -> Result<f32, &'static str> {
        // 模拟温度波动 ±0.5°C
        let base = self.base_temp.load(Ordering::Relaxed) as f32 / 10.0;
        let noise = (rand::random::<u8>() % 10) as f32 / 10.0 - 0.5;
        Ok(base + noise)
    }
}

/// 模拟风扇控制器
pub struct MockFanController {
    duty: AtomicU8,       // PWM 占空比 (0-100)
    target_rpm: AtomicU16, // 目标转速
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

/// 模拟 CPU 温度传感器
pub struct MockCpuTempSensor;

impl CpuTempSensor for MockCpuTempSensor {
    fn read_celsius(&self) -> Result<i32, &'static str> {
        // 模拟 CPU 温度在 38-48°C 之间波动
        Ok(38 + (rand::random::<u8>() % 11) as i32)
    }
}

// ============================================
// 真机硬件接口预留（未实现）
// ============================================

// TODO: 真机实现示例
// 
// pub struct Esp32TempSensor {
//     i2c: esp_idf_hal::i2c::Master<...>,
//     address: u8,
// }
// 
// impl TemperatureSensor for Esp32TempSensor {
//     fn read_celsius(&self) -> Result<f32, &'static str> {
//         // 从 I2C 读取温度传感器数据
//         let mut buf = [0u8; 2];
//         self.i2c.read(self.address, &mut buf)
//             .map_err(|_| "I2C read error")?;
//         
//         // 解析传感器数据（根据实际传感器型号）
//         let raw = u16::from_be_bytes(buf);
//         let celsius = (raw as f32) * 0.0625; // 示例转换
//         Ok(celsius)
//     }
// }
// 
// pub struct Esp32FanController {
//     pwm: esp_idf_hal::ledc::LedcChannel<...>,
//     rpm_pin: esp_idf_hal::gpio::PinDriver<...>,
// }
// 
// impl FanController for Esp32FanController {
//     fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str> {
//         let duty = (percentage as u32 * self.pwm.get_max_duty() / 100) as u32;
//         self.pwm.set_duty(duty)
//             .map_err(|_| "PWM set duty error")
//     }
//     
//     fn read_rpm(&self) -> Result<u16, &'static str> {
//         // 从 TACH 引脚读取转速脉冲
//         // 实现细节取决于风扇型号（通常是脉冲计数）
//         Ok(0) // TODO: 实现转速读取
//     }
// }

// ============================================
// 应用状态管理
// ============================================

/// 风扇控制系统状态
pub struct FanControlState {
    // 传感器数据
    temperature: f32,
    temp_history: Vec<f32>,  // 用于计算趋势
    
    // 风扇数据
    fan_rpm: u16,
    fan_duty: u8,
    
    // 系统状态
    cpu_temp: i32,
    uptime_secs: u64,
    
    // 硬件接口（trait objects）
    temp_sensor: Box<dyn TemperatureSensor>,
    fan_controller: Box<dyn FanController>,
    cpu_sensor: Box<dyn CpuTempSensor>,
    
    // 计时器
    start_time: Instant,
    last_update: Instant,
}

impl FanControlState {
    /// 创建新的状态管理器（使用模拟硬件）
    pub fn new_mock() -> Self {
        Self {
            temperature: 28.5,
            temp_history: vec![28.5; 10],
            fan_rpm: 2450,
            fan_duty: 75,
            cpu_temp: 42,
            uptime_secs: 0,
            temp_sensor: Box::new(MockTempSensor::new(28.5)),
            fan_controller: Box::new(MockFanController::new()),
            cpu_sensor: Box::new(MockCpuTempSensor),
            start_time: Instant::now(),
            last_update: Instant::now(),
        }
    }
    
    // TODO: 真机版本
    // pub fn new_hardware(
    //     temp_sensor: Box<dyn TemperatureSensor>,
    //     fan_controller: Box<dyn FanController>,
    //     cpu_sensor: Box<dyn CpuTempSensor>,
    // ) -> Self {
    //     Self {
    //         temperature: 25.0,
    //         temp_history: vec![25.0; 10],
    //         fan_rpm: 0,
    //         fan_duty: 0,
    //         cpu_temp: 0,
    //         uptime_secs: 0,
    //         temp_sensor,
    //         fan_controller,
    //         cpu_sensor,
    //         start_time: Instant::now(),
    //         last_update: Instant::now(),
    //     }
    // }
    
    /// 更新所有传感器数据
    pub fn update(&mut self) -> Result<(), &'static str> {
        let now = Instant::now();
        
        // 更新运行时间
        self.uptime_secs = now.duration_since(self.start_time).as_secs();
        
        // 读取温度传感器
        match self.temp_sensor.read_celsius() {
            Ok(temp) => {
                self.temperature = temp;
                // 保留最近 10 个温度值用于趋势计算
                self.temp_history.push(temp);
                if self.temp_history.len() > 10 {
                    self.temp_history.remove(0);
                }
            }
            Err(e) => eprintln!("温度读取失败: {}", e),
        }
        
        // 读取风扇转速
        match self.fan_controller.read_rpm() {
            Ok(rpm) => self.fan_rpm = rpm,
            Err(e) => eprintln!("转速读取失败: {}", e),
        }
        
        // 读取 CPU 温度
        match self.cpu_sensor.read_celsius() {
            Ok(temp) => self.cpu_temp = temp,
            Err(e) => eprintln!("CPU 温度读取失败: {}", e),
        }
        
        self.last_update = now;
        Ok(())
    }
    
    /// 设置风扇档位
    pub fn set_fan_speed(&mut self, percentage: i32) -> Result<(), &'static str> {
        let duty = if percentage < 0 {
            // AUTO 模式：根据温度自动调节
            self.calculate_auto_duty()
        } else {
            percentage.min(100) as u8
        };
        
        self.fan_duty = duty;
        self.fan_controller.set_duty_cycle(duty)
    }
    
    /// AUTO 模式：根据温度计算风扇档位
    fn calculate_auto_duty(&self) -> u8 {
        // 温度-档位映射表
        // < 30°C: 25%
        // 30-40°C: 25-50% (线性插值)
        // 40-50°C: 50-75% (线性插值)
        // 50-60°C: 75-100% (线性插值)
        // > 60°C: 100%
        
        if self.temperature < 30.0 {
            25
        } else if self.temperature < 40.0 {
            25 + ((self.temperature - 30.0) * 2.5) as u8
        } else if self.temperature < 50.0 {
            50 + ((self.temperature - 40.0) * 2.5) as u8
        } else if self.temperature < 60.0 {
            75 + ((self.temperature - 50.0) * 2.5) as u8
        } else {
            100
        }
    }
    
    /// 计算温度趋势 (°C/分钟)
    pub fn get_temp_trend(&self) -> f32 {
        if self.temp_history.len() < 2 {
            return 0.0;
        }
        
        let first = self.temp_history[0];
        let last = self.temp_history[self.temp_history.len() - 1];
        let delta = last - first;
        
        // 假设历史记录跨度约 10 秒，转换为每分钟
        delta * 6.0
    }
    
    /// 计算功率估算 (W)
    pub fn get_power_estimate(&self) -> f32 {
        // 简单估算：假设风扇最大功耗 8W
        (self.fan_duty as f32 / 100.0) * 8.0
    }
    
    /// 格式化运行时间为 HH:MM:SS
    pub fn format_uptime(&self) -> String {
        let hours = self.uptime_secs / 3600;
        let minutes = (self.uptime_secs % 3600) / 60;
        let seconds = self.uptime_secs % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    }
}

// ============================================
// CRT 上电动画控制
// ============================================

/// CRT 上电动画序列
pub async fn run_crt_powerup(ui: &slint::Weak<FanControlUI>) {
    let modules = [
        ("panel-1-on", 0),
        ("panel-2-on", 300),
        ("panel-3-on", 600),
        ("panel-4-on", 900),
        ("panel-5-on", 1200),
        ("panel-6-on", 1500),
    ];
    
    for (property, delay_ms) in modules {
        slint::spawn_local(async move {
            slint::Timer::single_shot(
                Duration::from_millis(delay_ms),
                move || {
                    if let Some(ui) = ui.upgrade() {
                        // TODO: 设置对应的 panel-X-on 属性为 true
                        // ui.set_property(property, true);
                    }
                }
            );
        }).unwrap();
    }
}

// ============================================
// 主程序入口
// ============================================

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    // 创建 UI 实例
    let ui = FanControlUI::new()?;
    
    // 创建状态管理器（模拟硬件）
    let state = Arc::new(std::sync::Mutex::new(FanControlState::new_mock()));
    
    // TODO: 真机版本
    // let state = Arc::new(std::sync::Mutex::new(
    //     FanControlState::new_hardware(
    //         Box::new(Esp32TempSensor::new(...)),
    //         Box::new(Esp32FanController::new(...)),
    //         Box::new(Esp32CpuTempSensor::new()),
    //     )
    // ));
    
    // 设置档位改变回调
    let state_clone = state.clone();
    ui.on_speed_changed(move |speed| {
        let mut state = state_clone.lock().unwrap();
        if let Err(e) = state.set_fan_speed(speed) {
            eprintln!("设置风扇档位失败: {}", e);
        }
    });
    
    // 启动数据更新循环（每秒更新）
    let ui_weak = ui.as_weak();
    let state_clone = state.clone();
    
    slint::spawn_local(async move {
        loop {
            slint::Timer::single_shot(Duration::from_secs(1), move || {
                let mut state = state_clone.lock().unwrap();
                
                // 更新传感器数据
                if let Err(e) = state.update() {
                    eprintln!("数据更新失败: {}", e);
                    return;
                }
                
                // 更新 UI
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_temperature(state.temperature);
                    ui.set_temp_trend(state.get_temp_trend());
                    ui.set_fan_rpm(state.fan_rpm as i32);
                    ui.set_fan_percentage(state.fan_duty as i32);
                    ui.set_fan_power(state.get_power_estimate());
                    ui.set_cpu_temp(state.cpu_temp);
                    ui.set_uptime(state.format_uptime().into());
                    ui.set_system_online(true);
                }
            });
            
            slint::Timer::single_shot(Duration::from_secs(1), || {}).await;
        }
    }).unwrap();
    
    // TODO: 启动 CRT 上电动画
    // run_crt_powerup(&ui.as_weak()).await;
    
    // 运行主循环
    ui.run()
}
