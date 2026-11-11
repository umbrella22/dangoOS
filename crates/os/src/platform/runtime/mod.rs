//! platform / runtime functionality.

#[cfg(target_os = "none")]
pub(crate) mod cores;
#[cfg(target_os = "none")]
pub(crate) mod crashlog;
#[cfg(target_os = "none")]
pub(crate) mod executors;
pub mod frequency;
#[cfg(any(target_os = "none", test))]
pub(crate) mod heap;
#[cfg(target_os = "none")]
pub(crate) mod libc;
#[cfg(any(target_os = "none", test, feature = "display-simulator"))]
pub(crate) mod memory;
#[cfg(target_os = "none")]
pub(crate) mod resident_budget;
pub mod scheduling;
#[cfg(target_os = "none")]
pub(crate) mod time;

pub(crate) mod buffers;
