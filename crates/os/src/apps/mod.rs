//! apps functionality.

#[cfg(target_os = "none")]
pub(crate) mod device;
pub mod lifecycle;
pub mod lua;
pub mod registry;
