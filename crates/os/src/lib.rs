//! Functional module tree for the host-tested models and device services.
//! Device hardware and executors are selected by `target_os = "none"`.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
extern crate alloc;

pub mod apps;
#[cfg(target_os = "none")]
pub mod boot;
pub mod companion;
pub mod console;
pub mod formats;
pub mod platform;
pub mod services;
pub mod shell;

pub use companion::behavior::Expression;
pub use companion::runtime::Runtime;
pub use companion::widget::{FoxControl, FoxVisual, FoxWidget, FRAME_INTERVAL_MS};
pub use shell::app::{next_ai_state, AppMsg, MosaicoApp, Screen, AI_STATES};
pub use shell::components::splash::{SplashControl, SplashWidget};

/// Process-wide LVGL test lock.
///
/// LVGL is a global singleton: two tests that call `rs_lvgl::init` /
/// `rs_lvgl::deinit` concurrently corrupt its state and crash. Every host test
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
