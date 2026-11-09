//! Consume the bootloader's one-use display handoff without touching panel GRAM.
use boot_contract::display::{Handoff, FIRST_STORE, WORDS};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static INHERITED: AtomicBool = AtomicBool::new(false);
static FIRST_US: AtomicU32 = AtomicU32::new(0);
static SELECTED_US: AtomicU32 = AtomicU32::new(0);

/// Only the panel constructor calls this, before any display task starts.
pub(crate) fn take() -> Option<Handoff> {
    let regs = esp_hal::peripherals::LP_SYS::regs();
    let words: [u32; WORDS] =
        core::array::from_fn(|i| regs.lp_store(FIRST_STORE + i).read().bits());
    // Consume before validation: partial or stale records cannot be retried.
    regs.lp_store(FIRST_STORE).write(|w| unsafe { w.bits(0) });
    Handoff::decode(words)
}

pub(crate) fn inherited(record: Handoff) {
    FIRST_US.store(record.first_frame_us, Ordering::Relaxed);
    SELECTED_US.store(record.selected_us, Ordering::Relaxed);
    INHERITED.store(true, Ordering::Release);
}

/// The loader's clock origin is distinct from the application's HAL clock.
pub fn status() -> (bool, u32, u32) {
    let inherited = INHERITED.load(Ordering::Acquire);
    (
        inherited,
        FIRST_US.load(Ordering::Relaxed),
        SELECTED_US.load(Ordering::Relaxed),
    )
}
