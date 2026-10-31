//! shell functionality.

pub mod app;
pub mod components;
pub mod lifecycle;
pub mod messages;
pub mod model;
pub mod navigation;
pub mod pages;
#[cfg(target_os = "none")]
pub(crate) mod runtime;
pub mod theme;
