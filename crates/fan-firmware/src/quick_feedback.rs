//! Install one UI feedback timer. start() replaces the prior single-shot run,
//! so repeated actions cannot let an older expiry clear newer feedback.
use alloc::rc::Rc;
use core::time::Duration;
use slint::ComponentHandle;

pub fn install(ui: &fan_ui::MosaicApp, timeout: Duration) {
    let timer = Rc::new(slint::Timer::default());
    ui.on_quick_feedback_requested({
        let weak = ui.as_weak();
        move || {
            let weak = weak.clone();
            timer.start(slint::TimerMode::SingleShot, timeout, move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_quick_feedback("".into());
                }
            });
        }
    });
}
