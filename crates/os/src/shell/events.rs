//! Congestion policy for native shell controls.
use super::messages::AppMsg;
use rs_lvgl_ui::{QueueOverflow, QueuePolicy};

/// Each absolute-value message identifies the shell's sole control for that
/// setting. Only adjacent values for the same setting may replace one another;
/// navigation, toggles, text input and commands preserve their ordering.
pub fn native_queue_policy() -> QueuePolicy<AppMsg> {
    QueuePolicy::new(QueueOverflow::DropOldest).coalesce(|old, new| match (old, new) {
        (AppMsg::AudioVolume(_), AppMsg::AudioVolume(value)) => Some(AppMsg::AudioVolume(*value)),
        (AppMsg::Brightness(_), AppMsg::Brightness(value)) => Some(AppMsg::Brightness(*value)),
        _ => None,
    })
}
