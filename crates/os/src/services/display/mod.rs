//! services / display functionality.

pub mod bitmap_font;
pub mod canvas_pool;
pub mod geometry;
#[cfg(target_os = "none")]
pub(crate) mod lvgl_port;
#[cfg(target_os = "none")]
pub(crate) mod plane;
#[cfg(target_os = "none")]
pub(crate) mod status;
