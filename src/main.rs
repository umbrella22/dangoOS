// EVE 风扇控制系统 - 主入口
// 作者: Kiro + Claude

mod service;
mod ui;

use slint::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// 导入服务层
use service::hardware::{FanControlState};
// use service::mock::{MockTempSensor, MockFanController, MockCpuTempSensor};  // 已在 new_mock() 内部使用

// 导入 UI 定义
slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    println!("🚀 EVE 风扇控制系统启动中...");
    println!("📦 硬件变体: {}", ui::VARIANT_NAME);
    
    // 创建 UI 实例
    let ui = FanControlUI::new()?;
    
    // 创建状态管理器（模拟硬件）
    // TODO: 真机时替换为真实硬件
    let state = Arc::new(Mutex::new(FanControlState::new_mock()));
    
    // TODO: 真机版本（硬件到货后取消注释）
    // let state = Arc::new(Mutex::new(
    //     FanControlState::new_hardware(
    //         Box::new(service::esp32::Esp32TempSensor::new(...)),
    //         Box::new(service::esp32::Esp32FanController::new(...)),
    //         Box::new(service::esp32::Esp32CpuTempSensor::new()),
    //     )
    // ));
    
    println!("✅ 硬件接口初始化完成（模拟模式）");
    
    // 设置档位改变回调
    let state_clone = state.clone();
    ui.on_speed_changed(move |speed| {
        let mut state = state_clone.lock().unwrap();
        if let Err(e) = state.set_fan_speed(speed) {
            eprintln!("❌ 设置风扇档位失败: {}", e);
        } else {
            println!("🔧 风扇档位已设置: {}%", if speed < 0 { "AUTO".to_string() } else { speed.to_string() });
        }
    });
    
    println!("✅ UI 回调已绑定");
    
    // 启动数据更新循环（每秒更新）
    let ui_weak = ui.as_weak();
    let state_clone = state.clone();
    
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            
            let mut state = state_clone.lock().unwrap();
            
            // 更新传感器数据
            if let Err(e) = state.update() {
                eprintln!("❌ 数据更新失败: {}", e);
                continue;
            }
            
            // 更新 UI
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_temperature(state.temperature as f32);
                ui.set_temp_trend(state.get_temp_trend());
                ui.set_fan_rpm(state.fan_rpm as i32);
                ui.set_fan_percentage(state.fan_duty as i32);
                ui.set_fan_power(state.get_power_estimate());
                ui.set_cpu_temp(state.cpu_temp);
                ui.set_uptime(state.format_uptime().into());
                ui.set_system_online(true);
            }
        }
    });
    
    println!("✅ 数据更新循环已启动");
    
    // TODO: 启动 CRT 上电动画
    // service::animation::run_crt_powerup(&ui.as_weak());
    
    println!("🎨 UI 渲染中...");
    
    // 运行主循环
    ui.run()
}
