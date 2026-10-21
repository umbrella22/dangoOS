//! Host-testable core for the Mosaico grok-ball screen.
//!
//! The crate contains the pinned upstream expression data, the ball engine,
//! the software rasterizer and the sensor affect engine. It has no LVGL,
//! `esp-hal` or RTOS dependency; host tests and the S31 firmware consume the
//! same code.
//!
//! The public surface is the union of the four modules. Work packages own one
//! module each so parallel changes do not collide:
//!
//! - [`data`] — generated upstream tables (G0.1).
//! - [`engine`] — expression, gaze, blink and body math (G0.2).
//! - [`raster`] — RGB565 polygon fill (G0.3).
//! - [`affect`] — sensor events and expression selection (H0).
//! - [`personality`] — human-feeling touch ladder, grab/drag and struggle (H3).
//! - [`skin`] — replaceable character appearances (cat/fox ears).
//! - [`sprite`] — bitmap animation playback for character art.
//! - [`types`] — shared types frozen for all of the above.

#![cfg_attr(not(test), no_std)]

pub mod affect;
pub mod data;
pub mod engine;
pub mod personality;
pub mod raster;
pub mod skin;
pub mod sprite;
pub mod types;

#[allow(unused_imports)]
pub use affect::*;
pub use data::*;
#[allow(unused_imports)]
pub use engine::*;
#[allow(unused_imports)]
pub use personality::*;
#[allow(unused_imports)]
pub use raster::*;
#[allow(unused_imports)]
pub use skin::*;
#[allow(unused_imports)]
pub use sprite::*;
pub use types::*;
