//! platform / runtime functionality.

#[cfg(target_os = "none")]
pub(crate) mod cores;
#[cfg(target_os = "none")]
pub(crate) mod crashlog;
#[cfg(target_os = "none")]
pub(crate) mod executors;
pub mod frequency;
#[cfg(target_os = "none")]
pub(crate) mod libc;
#[cfg(target_os = "none")]
pub(crate) mod memory;
#[cfg(target_os = "none")]
pub(crate) mod resident_budget;
pub mod scheduling;
#[cfg(target_os = "none")]
pub(crate) mod time;
