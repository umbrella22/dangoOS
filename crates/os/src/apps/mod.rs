//! apps functionality.

#[cfg(target_os = "none")]
pub(crate) mod agent;
#[cfg(target_os = "none")]
pub(crate) mod device;
pub mod lifecycle;
#[cfg(feature = "lua-engine")]
pub mod lua;
pub mod native_ui;
pub mod registry;
#[cfg(target_os = "none")]
pub(crate) mod runtime;
#[cfg(feature = "ts-engine")]
pub mod ts;

#[cfg(target_os = "none")]
mod host_device;
