//! Slint event loop and bindings for the desktop simulator.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use fan_ui::{
    boot::{BootAnimation, BootFrame, FRAME_INTERVAL},
    history::TemperatureHistory,
    FanControlUI,
};
use slint::{ComponentHandle, Timer, TimerMode};

use crate::simulation::FanControlState;

pub fn run() -> Result<(), slint::PlatformError> {
    println!(
        "ikaros desktop preview ({}×{})",
        fan_ui::WIDTH,
        fan_ui::HEIGHT
    );
    let ui = FanControlUI::new()?;
    let state = Rc::new(RefCell::new(FanControlState::new_mock()));
    let mut temp_history = TemperatureHistory::default();
    ui.set_temp_history(temp_history.model());

    let controls = Rc::clone(&state);
    ui.on_speed_changed(move |speed| {
        if let Err(error) = controls.borrow_mut().set_fan_speed(speed) {
            eprintln!("设置风扇档位失败: {error}");
        }
    });

    // Slint component access stays on the event-loop thread.
    let weak_ui = ui.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let mut state = state.borrow_mut();
        if let Err(error) = state.update() {
            eprintln!("数据更新失败: {error}");
            return;
        }
        if let Some(ui) = weak_ui.upgrade() {
            ui.set_temperature(state.temperature);
            ui.set_temp_trend(state.get_temp_trend());
            temp_history.set_samples(&state.temp_history);
            ui.set_fan_rpm(state.fan_rpm as i32);
            ui.set_fan_percentage(state.fan_duty as i32);
            ui.set_fan_power(state.get_power_estimate());
            ui.set_cpu_temp(state.cpu_temp);
            ui.set_uptime(state.format_uptime().into());
            ui.set_system_online(true);
        }
    });

    ui.show()?;
    let boot_timer = Rc::new(Timer::default());
    let weak_timer = Rc::downgrade(&boot_timer);
    let boot_ui = ui.as_weak();
    let boot_started = Instant::now();
    let mut boot = BootAnimation::default();
    boot_timer.start(TimerMode::Repeated, FRAME_INTERVAL, move || {
        if let Some(ui) = boot_ui.upgrade() {
            match boot.advance(boot_started.elapsed()) {
                Some(BootFrame::Progress(progress)) => ui.set_boot_progress(progress),
                Some(BootFrame::Finished) => {
                    ui.set_booting(false);
                    if let Some(timer) = weak_timer.upgrade() {
                        timer.stop();
                    }
                }
                None => {}
            }
        }
    });

    ui.run()
}
