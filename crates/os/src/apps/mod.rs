//! apps functionality.

#[cfg(target_os = "none")]
pub(crate) mod device;
pub mod lifecycle;
#[cfg(feature = "lua-engine")]
pub mod lua;
pub mod native_ui;
pub mod registry;
#[cfg(target_os = "none")]
pub(crate) mod runtime;
#[cfg(all(target_os = "none", feature = "ts-engine"))]
mod ts;
#[cfg(all(feature = "ts-engine", any(target_os = "none", test)))]
mod ts_scheduling;

#[cfg(target_os = "none")]
mod host_device;
#[cfg(feature = "ts-engine")]
pub mod ts_host;

#[cfg(feature = "ts-engine")]
pub mod ts_graphics;
