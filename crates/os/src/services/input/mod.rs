//! services / input functionality.

#[cfg(target_os = "none")]
pub(crate) mod sensors;
pub mod snapshot;
#[cfg(target_os = "none")]
pub(crate) mod touch;
