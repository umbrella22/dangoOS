//! platform / storage functionality.

#[cfg(target_os = "none")]
pub(crate) mod nand;
#[cfg(target_os = "none")]
pub(crate) mod nor;
