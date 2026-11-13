//! shell / pages / settings functionality.

pub mod model;
#[cfg(any(target_os = "none", test))]
pub(crate) mod operations;
pub(crate) mod update;
pub mod view;
