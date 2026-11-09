//! Native AI application's public device tools. Voice supplies transport and
//! current-task storage; the UI task applies preferences through its normal path.
use alloc::string::{String, ToString};
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{with_timeout, Duration, Timer};
pub use voice_session::agent::Action;
static NEXT: AtomicU32 = AtomicU32::new(1);
static ACTIVE: AtomicU32 = AtomicU32::new(0);
static ACCEPTED: AtomicU32 = AtomicU32::new(0);
static CONTROL: Signal<CriticalSectionRawMutex, (u32, Action)> = Signal::new();
struct Ticket(u32);
impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = ACTIVE.compare_exchange(self.0, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}
pub(crate) fn take_control() -> Option<(u32, Action)> {
    CONTROL
        .try_take()
        .filter(|(id, _)| ACTIVE.load(Ordering::Acquire) == *id)
}
pub(crate) fn accepted(id: u32) {
    ACCEPTED.store(id, Ordering::Release);
}
fn status() -> String {
    let audio = crate::services::audio::task::snapshot();
    let hardware = crate::services::input::sensors::lua_snapshot();
    let battery = (hardware.valid & 32 != 0).then_some(hardware.soc);
    let voltage = (hardware.valid & 32 != 0).then_some(hardware.voltage_mv);
    serde_json::json!({"ok":true,"volume_percent":audio.volume,"muted":audio.muted,
        "speaker_output_ceiling_db":crate::services::audio::volume::OUTPUT_CEILING_DB,
        "brightness_percent":crate::services::display::plane::brightness(),
        "battery_percent":battery,"battery_mv":voltage})
    .to_string()
}
pub(crate) async fn execute(action: Result<Action, &'static str>) -> String {
    let action = match action {
        Ok(action) => action,
        Err(error) => return serde_json::json!({"ok":false,"error":error}).to_string(),
    };
    if action == Action::Status {
        return status();
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed).wrapping_add(1).max(1);
    if ACTIVE
        .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return serde_json::json!({"ok":false,"error":"device_control_busy"}).to_string();
    }
    let _ticket = Ticket(id);
    CONTROL.signal((id, action));
    let applied = with_timeout(Duration::from_secs(3), async {
        while ACCEPTED.load(Ordering::Acquire) != id {
            Timer::after_millis(10).await;
        }
        if let Action::Brightness(percent) = action {
            while crate::services::display::plane::brightness() != Some(percent) {
                Timer::after_millis(10).await;
            }
        }
    })
    .await
    .is_ok();
    if applied {
        status()
    } else {
        serde_json::json!({"ok":false,"error":"device_apply_timeout"}).to_string()
    }
}
