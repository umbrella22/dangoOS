//! TypeScript application execution and OS host adapters.
#[cfg(target_os = "none")]
pub(crate) mod device;
pub mod graphics;
pub mod host;
#[cfg(any(target_os = "none", test))]
mod scheduling;
