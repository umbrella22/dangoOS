//! services / power functionality.

pub mod battery;
#[cfg(target_os = "none")]
pub(crate) mod battery_task;
#[cfg(target_os = "none")]
pub(crate) mod button_task;
pub mod buttons;
#[cfg(target_os = "none")]
pub(crate) mod coordinator;
#[cfg(target_os = "none")]
pub(crate) mod performance;
pub mod standby;
pub mod wake;
