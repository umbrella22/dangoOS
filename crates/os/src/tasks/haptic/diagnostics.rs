//! Read-only, opt-in receipts for the motor's RMT channel and GPIO route.
//! No interrupt acknowledgement, register writes, or additional waits.

use core::sync::atomic::{AtomicU8, Ordering};

static PATTERNS: AtomicU8 = AtomicU8::new(0);
const PATTERN_LIMIT: u8 = 10;

/// Limit logs to the first ten patterns, including startup module feedback.
/// Only the first burst of each selected pattern is observed.
pub(super) fn next_pattern() -> Option<u8> {
    PATTERNS
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            (n < PATTERN_LIMIT).then_some(n + 1)
        })
        .ok()
        .map(|n| n + 1)
}

pub(super) fn record(pattern: Option<u8>, burst: u8, stage: &str) {
    let Some(pattern) = pattern else { return };
    // SAFETY: create only temporary read-only PAC views. The haptic task owns
    // RMT channel 0; GPIO8 belongs to its output route. Touch is owned by the
    // sensor task, so these sequential reads are observational, not an atomic
    // cross-peripheral snapshot. Reading these registers has no clear-on-read
    // side effects; in particular INT_RAW is read, never INT_CLR.
    let (conf, status, irq, oe8, mux, matrix, scan, touch_out, ram0, ram1, limit) = unsafe {
        let rmt = esp32s31::RMT::steal();
        let gpio = esp32s31::GPIO::steal();
        let iomux = esp32s31::IO_MUX::steal();
        let touch = esp32s31::LP_TOUCH_AON::steal();
        (
            rmt.ch_tx_conf0(0).read().bits(),
            rmt.ch_tx_status(0).read().bits(),
            rmt.int_raw().read().bits(),
            (gpio.enable().read().bits() >> 8) & 1,
            iomux.gpio(8).read().bits(),
            gpio.func_out_sel_cfg(8).read().bits(),
            touch.scan_ctrl1().read().touch_scan_pad_map().bits(),
            touch.filter2().read().touch_outen().bits(),
            // esp-hal bb239a0 esp-metadata/devices/esp32s31/soc.toml:
            // RMT RAM base 0x20355800, 48 u32 words per channel; ch0 starts here.
            core::ptr::read_volatile(0x2035_5800 as *const u32),
            core::ptr::read_volatile(0x2035_5804 as *const u32),
            rmt.ch_tx_lim(0).read().bits(),
        )
    };
    log::info!(
        "hd p={pattern} b={burst} {stage} t={} c={conf:08x} s={status:08x} i={irq:08x} oe8={oe8} m={mux:08x} o={matrix:08x} tp={scan:04x}/{touch_out:04x}",
        super::now_ms()
    );
    // Separate short line: the console's line cap must not truncate RAM/limit.
    log::info!("hdx p={pattern} b={burst} {stage} ram={ram0:08x}/{ram1:08x} lim={limit:08x}");
}
