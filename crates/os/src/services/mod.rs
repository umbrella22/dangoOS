//! services functionality.

pub mod audio;
pub mod camera;
pub mod display;
#[cfg(target_os = "none")]
pub(crate) mod feedback;
pub mod input;
pub mod power;
pub mod radio;
pub mod settings;
pub mod timer;
pub mod update;
