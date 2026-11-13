//! Early reserve gate and independent 1 Hz protection, before slow sensor probes.
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_time::{Instant, Timer};
use {crate::services::power::battery::Policy, crate::services::power::battery::Reading};
static BOOT_DONE: AtomicBool = AtomicBool::new(false);

pub async fn wait_boot() -> bool {
    while !BOOT_DONE.load(Ordering::Acquire) {
        Timer::after_millis(5).await;
    }
    !crate::services::power::button_task::low_battery_requested()
}
fn observe(policy: &mut Policy, reading: Option<Reading>) {
    if policy.observe(Instant::now().as_millis(), reading) {
        if let Some(reading) = reading {
            log::warn!(
                "power: reserve cutoff mv={} avg_ma={}",
                reading.voltage_mv,
                reading.current_ma
            );
        }
        crate::services::power::button_task::request_low_battery();
    }
}
pub async fn boot_check() {
    let mut policy = Policy::new(200);
    for n in 0..3 {
        let reading = crate::platform::drivers::battery::read_power().await.ok();
        observe(&mut policy, reading);
        if n == 0 && reading.is_none_or(|r| r.voltage_mv > crate::services::power::battery::LOW_MV)
        {
            break;
        }
        if n < 2 {
            Timer::after_millis(110).await;
        }
    }
    BOOT_DONE.store(true, Ordering::Release);
}
#[embassy_executor::task]
pub async fn task() {
    let mut policy = Policy::new(3000);
    loop {
        Timer::after_millis(1000).await;
        let reading = crate::platform::drivers::battery::read_power().await.ok();
        observe(&mut policy, reading);
    }
}
