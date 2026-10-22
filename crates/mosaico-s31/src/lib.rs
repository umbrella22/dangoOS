//! The host-testable half of the Mosaico S31 firmware.
//!
//! Everything here is `no_std` + `alloc` and builds for the host as well as
//! the device, so `cargo test -p mosaico-s31` covers it without hardware. The
//! binary (`src/main.rs`) adds the board, the drivers and the Embassy tasks.
//!
//! - [`behavior`]: what the fox feels. Sensors, buttons and touch in; one
//!   expression and one haptic pattern out.
//! - [`fox`]: what the fox looks like. Palette sprites, the per-expression
//!   animation and the renderer that draws a frame into an RGB565 canvas.
//! - [`ui`]: the LVGL screen tree, the fox widget and the canvas pool shared
//!   with the panel upload.
//! - [`telemetry`]: latency and display counters for the console.
//!
//! Data flow per frame: `behavior::Runtime` -> `ui::fox_widget::FoxControl`
//! -> `fox::FoxAnimator` -> `fox::FoxRenderer` -> `ui::canvas_pool` -> the
//! firmware's plane task -> panel.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

pub mod behavior;
pub mod buttons;
pub mod fox;
pub mod hub;
pub mod telemetry;
pub mod ui;

pub use behavior::{Expression, Runtime};
pub use ui::fox_widget::{FoxControl, FoxVisual, FoxWidget, FRAME_INTERVAL_MS};
pub use ui::splash::{SplashControl, SplashWidget};
pub use ui::{next_ai_state, AppMsg, MosaicoApp, Screen, AI_STATES};

/// Process-wide LVGL test lock.
///
/// LVGL is a global singleton: two tests that call `lvgl::init` /
/// `lvgl::deinit` concurrently corrupt its state and crash. Every host test
/// that touches LVGL takes this one lock, so `cargo test` may use any thread
/// count.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static LVGL_LOCK: Mutex<()> = Mutex::new(());

    /// Serialize the calling test against every other LVGL test.
    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        LVGL_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub mod lua_lvgl;
