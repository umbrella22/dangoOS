//! Host-safe library surface for the Mosaico S31 firmware crate.
//!
//! The binary (`src/main.rs`) keeps its ESP-only modules. This library is
//! `no_std` + `alloc` and contains only code that also builds on the host, so
//! the ball widget, the boot splash, the input queue and the latency sample
//! math can be exercised without hardware.
//!
//! `present.rs` is compiled by the binary because it depends on
//! `crate::framebuffer` and `crate::memory`, whose host test modules need
//! imports owned by R1.1; exporting it here would break `cargo test --lib`
//! without cross-package edits. The source is intentionally not duplicated.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;
// The firmware binary compiles `ui.rs` and `ui_task.rs` as modules of the
// binary crate, whose `main.rs` does not declare `ball_widget`. Referring to
// the package library by its own name keeps one `BallWidget` type in both
// compilation units.
extern crate self as mosaico_s31;

pub mod ball_widget;
pub mod boot_splash;
pub mod entrance;
pub mod input_queue;
pub mod latency;
pub mod runtime;
pub mod splash_font;
pub mod splash_widget;
pub mod ui;

pub use ball_widget::{BallControl, BallVisual, BallWidget, FRAME_INTERVAL_MS};
pub use runtime::BallRuntime;
pub use splash_widget::{SplashControl, SplashWidget};
pub use ui::{next_ai_state, BallApp, BallMsg, Screen, AI_STATES};

/// Process-wide LVGL test lock.
///
/// LVGL is a global singleton: two tests that call `lvgl::init` /
/// `lvgl::deinit` concurrently corrupt its state and crash. Every host test
/// that touches LVGL takes this one lock, so `cargo test` may use any thread
/// count.
#[cfg(test)]
pub(crate) mod test_support {
    extern crate std;

    use std::sync::{Mutex, MutexGuard};

    static LVGL_LOCK: Mutex<()> = Mutex::new(());

    /// Serialize the calling test against every other LVGL test.
    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        LVGL_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
