//! Diagnostic accumulation for the current UI loop; callbacks never log.

use core::cell::Cell;

#[derive(Clone, Copy, Default)]
pub(crate) struct Stats {
    pub sensors: u32,
    pub balance_us: u64,
    pub native_us: u64,
}

static STATS: critical_section::Mutex<Cell<Stats>> =
    critical_section::Mutex::new(Cell::new(Stats {
        sensors: 0,
        balance_us: 0,
        native_us: 0,
    }));

fn with(f: impl FnOnce(&mut Stats)) {
    critical_section::with(|cs| {
        let cell = STATS.borrow(cs);
        let mut stats = cell.get();
        f(&mut stats);
        cell.set(stats);
    });
}

pub(crate) fn take() -> Stats {
    critical_section::with(|cs| STATS.borrow(cs).take())
}

pub(crate) fn sensor(balance_us: u64, native_us: u64) {
    with(|s| {
        s.sensors += 1;
        s.balance_us += balance_us;
        s.native_us += native_us;
    });
}
