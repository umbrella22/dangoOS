//! services / display functionality.

pub mod bitmap_font;
pub mod canvas_pool;
pub mod damage;
#[cfg(feature = "display-metrics")]
pub(crate) mod diagnostics;
pub mod geometry;
#[cfg(any(target_os = "none", test, feature = "display-simulator"))]
pub mod lvgl_port;
#[cfg(target_os = "none")]
pub(crate) mod plane;
#[cfg(any(test, feature = "display-simulator"))]
pub mod simulator;
#[cfg(target_os = "none")]
pub(crate) mod status;
