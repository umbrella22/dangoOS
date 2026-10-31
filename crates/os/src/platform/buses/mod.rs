//! platform / buses functionality.

#[cfg(target_os = "none")]
pub(crate) mod shared_i2c;
