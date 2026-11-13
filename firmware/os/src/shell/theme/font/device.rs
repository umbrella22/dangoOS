//! Services prepares owned data. Only UI activates it, between render passes.
use crate::services::resources::fonts::Fonts;
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

static PREPARED: Channel<CriticalSectionRawMutex, Option<Fonts>, 1> = Channel::new();
static ACTIVE: AtomicPtr<Fonts> = AtomicPtr::new(core::ptr::null_mut());
static RESOLVED: AtomicBool = AtomicBool::new(false);

pub async fn prepare() {
    let fonts = match ::fs::global::with(|vfs| Ok(Fonts::load(vfs))) {
        Ok(Ok(fonts)) => Some(fonts),
        Ok(Err(error)) => {
            log::warn!("ui fonts: {error}");
            None
        }
        Err(error) => {
            log::warn!("ui fonts: VFS {error}");
            None
        }
    };
    PREPARED.send(fonts).await;
}
pub(super) fn active() -> Option<&'static Fonts> {
    let p = ACTIVE.load(Ordering::Acquire);
    // SAFETY: published by UI once, immutable and retained until reset.
    unsafe { p.as_ref() }
}
pub fn recovery_active() -> bool {
    RESOLVED.load(Ordering::Acquire) && active().is_none()
}
/// Called by UI once the boot scan completes, between render passes. No file IO.
pub fn activate_prepared() {
    let Ok(fonts) = PREPARED.try_receive() else {
        return;
    };
    if let Some(fonts) = fonts {
        let p = Box::into_raw(Box::new(fonts));
        if ACTIVE
            .compare_exchange(
                core::ptr::null_mut(),
                p,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_err()
        {
            // SAFETY: this unpublished candidate is still exclusively ours.
            drop(unsafe { Box::from_raw(p) });
        }
        // SAFETY: UI is the sole LVGL owner. No glyph lookup is in progress;
        // discard cached fallback advances and invalidate existing text layout.
        unsafe { rs_lvgl::sys::lv_obj_report_style_change(core::ptr::null_mut()) };
        log::info!(
            "ui fonts: loaded {} bytes from resource set {}",
            crate::services::resources::fonts::BYTES,
            crate::services::resources::fonts::ID
        );
    } else {
        log::warn!("ui fonts: recovery subset; install the matching resource bundle and restart");
    }
    RESOLVED.store(true, Ordering::Release);
}
