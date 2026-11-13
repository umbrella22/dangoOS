//! Display readiness used by startup health and standby admission.
use core::sync::atomic::AtomicBool;

/// Set once a complete frame has been presented and released.
pub static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
/// Set once panel brightness has been restored after a complete frame.
pub static BRIGHTNESS_RESTORED: AtomicBool = AtomicBool::new(false);
