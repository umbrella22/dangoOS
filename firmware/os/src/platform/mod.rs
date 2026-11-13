//! platform functionality.

#[cfg(target_os = "none")]
pub(crate) mod board;
#[cfg(target_os = "none")]
pub(crate) mod buses;
#[cfg(target_os = "none")]
pub(crate) mod drivers;
pub mod runtime;
#[cfg(target_os = "none")]
pub(crate) mod storage;
