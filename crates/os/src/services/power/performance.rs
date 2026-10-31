//! One governor for the shared CPU clock; no per-page tasks or allocations.
use crate::platform::runtime::frequency::{device, Policy};
use embassy_time::{Instant, Timer};

#[embassy_executor::task]
pub async fn task() {
    device::enable();
    let mut policy = Policy::new(Instant::now().as_millis());
    loop {
        let audio = crate::services::audio::task::snapshot().state;
        let background_busy = crate::services::camera::task::busy()
            || matches!(
                audio,
                crate::services::audio::State::Starting
                    | crate::services::audio::State::Playing
                    | crate::services::audio::State::Recording
            )
            || crate::services::update::task::busy()
            || crate::services::update::ota::device::blocks_standby();
        let demand = device::demand();
        let target = policy.target(Instant::now().as_millis(), demand, background_busy);
        device::apply(target, demand);
        Timer::after_millis(100).await;
    }
}
