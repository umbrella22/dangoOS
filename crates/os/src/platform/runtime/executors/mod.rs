//! platform / runtime / executors functionality.

#[cfg(target_os = "none")]
pub(crate) mod services;
#[cfg(all(target_os = "none", feature = "dual-core-ui"))]
pub(crate) mod ui;
