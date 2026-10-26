//! Optional native-Lua profiling. All times are microseconds from one clock.
//! Handler includes flush; transfer includes packing and SPI awaits. Those
//! containing measurements must not be added to their contained measurements.
//! Fixed storage, no allocation and no logging at measurement points.

pub const SLOW_CAPACITY: usize = 12;
const SLOW_US: u64 = 50_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metric {
    pub n: u32,
    pub us: u64,
    pub max: u64,
}
impl Metric {
    const ZERO: Self = Self {
        n: 0,
        us: 0,
        max: 0,
    };
    fn add(&mut self, us: u64) {
        self.n += 1;
        self.us += us;
        self.max = self.max.max(us);
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub loops: Metric,
    pub work: Metric,
    pub handler: Metric,
    pub flush: Metric,
    pub rpc: Metric,
    pub queue: Metric,
    pub te: Metric,
    pub transfer: Metric,
    pub lease: Metric,
    pub submitted: u32,
    pub completed: u32,
    pub cancelled: u32,
    pub busy_skips: u32,
    pub busy_us: u64,
    pub flush_pixels: u64,
    pub damage_pixels: u64,
}
impl Stats {
    const ZERO: Self = Self {
        loops: Metric::ZERO,
        work: Metric::ZERO,
        handler: Metric::ZERO,
        flush: Metric::ZERO,
        rpc: Metric::ZERO,
        queue: Metric::ZERO,
        te: Metric::ZERO,
        transfer: Metric::ZERO,
        lease: Metric::ZERO,
        submitted: 0,
        completed: 0,
        cancelled: 0,
        busy_skips: 0,
        busy_us: 0,
        flush_pixels: 0,
        damage_pixels: 0,
    };
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pub id: u32,
    pub session: u32,
    pub handler_start: u64,
    pub handler_end: u64,
    pub flush_us: u64,
    pub flush_n: u32,
    pub flush_pixels: u64,
    pub rect: [u16; 4],
    pub leased: u64,
    pub submit: u64,
    pub pickup: u64,
    pub te_end: u64,
    pub transfer_end: u64,
    pub release: u64,
}
impl Frame {
    const ZERO: Self = Self {
        id: 0,
        session: 0,
        handler_start: 0,
        handler_end: 0,
        flush_us: 0,
        flush_n: 0,
        flush_pixels: 0,
        rect: [0; 4],
        leased: 0,
        submit: 0,
        pickup: 0,
        te_end: 0,
        transfer_end: 0,
        release: 0,
    };
}
pub struct Report {
    /// Window metrics cover all native sessions in this inclusive range.
    pub first_session: u32,
    pub session: u32,
    pub exit_session: u32,
    pub active: bool,
    pub window_start: u64,
    pub at: u64,
    pub stats: Stats,
    /// Cumulative process totals, printed only at native-session exit.
    pub totals: Option<Stats>,
    /// Last slow frames, retained through startup until exit (oldest first).
    pub slow: [Frame; SLOW_CAPACITY],
    pub slow_len: usize,
    pub slow_seen: u32,
}
pub struct Profile {
    active: bool,
    session: u32,
    next_id: u32,
    window_start: Option<u64>,
    first_session: u32,
    exit_session: u32,
    last_loop: Option<u64>,
    busy_since: Option<u64>,
    handler: Frame,
    handler_open: bool,
    current: Option<Frame>,
    window: Stats,
    total: Stats,
    exit_pending: bool,
    slow: [Frame; SLOW_CAPACITY],
    slow_next: usize,
    slow_len: usize,
    slow_seen: u32,
}
impl Profile {
    pub const fn new() -> Self {
        Self {
            active: false,
            session: 0,
            next_id: 0,
            window_start: None,
            first_session: 0,
            exit_session: 0,
            last_loop: None,
            busy_since: None,
            handler: Frame::ZERO,
            handler_open: false,
            current: None,
            window: Stats::ZERO,
            total: Stats::ZERO,
            exit_pending: false,
            slow: [Frame::ZERO; SLOW_CAPACITY],
            slow_next: 0,
            slow_len: 0,
            slow_seen: 0,
        }
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn set_active(&mut self, active: bool, now: u64) {
        if active == self.active {
            return;
        }
        self.active = active;
        self.last_loop = None;
        self.handler = Frame::ZERO;
        self.handler_open = false;
        if active {
            self.session += 1;
            if self.current.is_none() && !self.exit_pending {
                self.window_start = Some(now);
                self.window = Stats::ZERO;
                self.first_session = self.session;
            }
            // Otherwise preserve late completion and explicitly mark a mixed
            // window. Totals are process-wide; each frame retains its session.
        } else {
            self.exit_pending = true;
            self.exit_session = self.session;
        }
    }
    pub fn loop_start(&mut self, now: u64) {
        if !self.active {
            return;
        }
        if let Some(last) = self.last_loop.replace(now) {
            self.window.loops.add(now.saturating_sub(last));
            self.total.loops.add(now.saturating_sub(last));
        }
    }
    pub fn work(&mut self, us: u64) {
        if self.active {
            self.window.work.add(us);
            self.total.work.add(us);
        }
    }
    pub fn rpc(&mut self, us: u64) {
        if self.active {
            self.window.rpc.add(us);
            self.total.rpc.add(us);
        }
    }
    pub fn begin_handler(&mut self, now: u64) {
        if self.active {
            self.handler_open = true;
            self.handler = Frame {
                handler_start: now,
                ..Frame::ZERO
            };
        }
    }
    pub fn end_handler(&mut self, now: u64) {
        if !self.active {
            return;
        }
        self.handler.handler_end = now;
        self.handler_open = false;
        let us = now.saturating_sub(self.handler.handler_start);
        self.window.handler.add(us);
        self.total.handler.add(us);
    }
    pub fn flush(&mut self, us: u64, pixels: u64) {
        if !self.active {
            return;
        }
        self.window.flush.add(us);
        self.total.flush.add(us);
        self.window.flush_pixels += pixels;
        self.total.flush_pixels += pixels;
        if self.handler_open {
            self.handler.flush_us += us;
            self.handler.flush_n += 1;
            self.handler.flush_pixels += pixels;
        }
    }
    pub fn busy_skip(&mut self) {
        if self.active {
            self.window.busy_skips += 1;
            self.total.busy_skips += 1;
        }
    }
    pub fn lease(&mut self, now: u64, rect: [u16; 4]) -> Option<u32> {
        if !self.active {
            return None;
        }
        debug_assert!(
            self.current.is_none(),
            "one shadow means one profiled lease"
        );
        self.next_id += 1;
        let f = Frame {
            id: self.next_id,
            session: self.session,
            leased: now,
            rect,
            ..self.handler
        };
        self.current = Some(f);
        self.handler = Frame::ZERO;
        self.busy_since = Some(now);
        Some(f.id)
    }
    pub fn submit(&mut self, id: u32, now: u64) {
        if let Some(f) = self.current.as_mut().filter(|f| f.id == id) {
            self.first_session = self.first_session.min(f.session);
            f.submit = now;
            let pixels =
                u64::from(f.rect[2] - f.rect[0] + 1) * u64::from(f.rect[3] - f.rect[1] + 1);
            self.window.submitted += 1;
            self.total.submitted += 1;
            self.window.damage_pixels += pixels;
            self.total.damage_pixels += pixels;
        }
    }
    pub fn pickup(&mut self, id: u32, now: u64) {
        if let Some(f) = self.current.as_mut().filter(|f| f.id == id) {
            self.first_session = self.first_session.min(f.session);
            f.pickup = now;
            let us = now.saturating_sub(f.submit);
            self.window.queue.add(us);
            self.total.queue.add(us);
        }
    }
    pub fn te_end(&mut self, id: u32, now: u64) {
        if let Some(f) = self.current.as_mut().filter(|f| f.id == id) {
            self.first_session = self.first_session.min(f.session);
            f.te_end = now;
            let us = now.saturating_sub(f.pickup);
            self.window.te.add(us);
            self.total.te.add(us);
        }
    }
    pub fn transfer_end(&mut self, id: u32, now: u64) {
        if let Some(f) = self.current.as_mut().filter(|f| f.id == id) {
            self.first_session = self.first_session.min(f.session);
            f.transfer_end = now;
            let us = now.saturating_sub(f.te_end);
            self.window.transfer.add(us);
            self.total.transfer.add(us);
        }
    }
    fn account_busy(&mut self, now: u64) {
        if let Some(start) = self.busy_since.replace(now) {
            if let Some(f) = self.current {
                self.first_session = self.first_session.min(f.session);
            }
            let us = now.saturating_sub(start);
            self.window.busy_us += us;
            self.total.busy_us += us;
        } else {
            self.busy_since = None;
        }
    }
    pub fn release(&mut self, id: u32, now: u64) {
        if self.current.as_ref().is_none_or(|f| f.id != id) {
            return;
        }
        self.first_session = self
            .first_session
            .min(self.current.as_ref().unwrap().session);
        self.account_busy(now);
        self.busy_since = None;
        let mut f = self.current.take().unwrap();
        f.release = now;
        let us = now.saturating_sub(f.leased);
        self.window.lease.add(us);
        self.total.lease.add(us);
        if f.transfer_end == 0 {
            self.window.cancelled += 1;
            self.total.cancelled += 1;
        } else {
            self.window.completed += 1;
            self.total.completed += 1;
        }
        if f.handler_start != 0 && now.saturating_sub(f.handler_start) >= SLOW_US {
            self.slow[self.slow_next] = f;
            self.slow_next = (self.slow_next + 1) % SLOW_CAPACITY;
            self.slow_len = (self.slow_len + 1).min(SLOW_CAPACITY);
            self.slow_seen += 1;
        }
    }
    /// Fixed wall-clock windows; explicitly reports zero completions while
    /// idle. An exit waits for the last lease before printing final totals.
    pub fn report(&mut self, now: u64) -> Option<Report> {
        let start = self.window_start?;
        if now.saturating_sub(start) < 1_000_000 {
            return None;
        }
        if !self.active && !self.exit_pending && self.current.is_none() {
            return None;
        }
        self.account_busy(now);
        let exit = self.exit_pending && self.current.is_none();
        let mut slow = [Frame::ZERO; SLOW_CAPACITY];
        let slow_len = if exit { self.slow_len } else { 0 };
        for (i, dst) in slow.iter_mut().take(slow_len).enumerate() {
            *dst = self.slow[(self.slow_next + SLOW_CAPACITY - self.slow_len + i) % SLOW_CAPACITY];
        }
        let r = Report {
            first_session: self.first_session,
            session: self.session,
            exit_session: self.exit_session,
            active: self.active,
            window_start: start,
            at: now,
            stats: self.window,
            totals: exit.then_some(self.total),
            slow,
            slow_len,
            slow_seen: self.slow_seen,
        };
        self.window = Stats::ZERO;
        self.window_start = Some(now);
        self.first_session = self.session;
        if exit {
            self.exit_pending = false;
            self.slow_len = 0;
            self.slow_next = 0;
        }
        Some(r)
    }
}

#[cfg(target_os = "none")]
mod device {
    use super::*;
    use core::cell::RefCell;
    static PROFILE: critical_section::Mutex<RefCell<Profile>> =
        critical_section::Mutex::new(RefCell::new(Profile::new()));
    pub fn with<R>(f: impl FnOnce(&mut Profile) -> R) -> R {
        critical_section::with(|cs| f(&mut PROFILE.borrow_ref_mut(cs)))
    }
    pub fn report(now: u64) {
        let Some(r) = with(|p| p.report(now)) else {
            return;
        };
        let s = r.stats;
        log::info!(
            "native-ui w={} s={}:{} dt={} a={} f={}/{}/{} busy={}/{} loop={}/{}/{} work={}/{}/{}",
            r.at,
            r.first_session,
            r.session,
            r.at - r.window_start,
            u8::from(r.active),
            s.submitted,
            s.completed,
            s.cancelled,
            s.busy_skips,
            s.busy_us,
            s.loops.n,
            s.loops.us,
            s.loops.max,
            s.work.n,
            s.work.us,
            s.work.max
        );
        log::info!(
            "native-draw w={} s={}:{} h={}/{}/{} f={}/{}/{} px={} dmg={} rpc={}/{}/{}",
            r.at,
            r.first_session,
            r.session,
            s.handler.n,
            s.handler.us,
            s.handler.max,
            s.flush.n,
            s.flush.us,
            s.flush.max,
            s.flush_pixels,
            s.damage_pixels,
            s.rpc.n,
            s.rpc.us,
            s.rpc.max
        );
        log::info!(
            "native-plane w={} s={}:{} q={}/{}/{} te={}/{}/{} tx={}/{}/{} lease={}/{}/{}",
            r.at,
            r.first_session,
            r.session,
            s.queue.n,
            s.queue.us,
            s.queue.max,
            s.te.n,
            s.te.us,
            s.te.max,
            s.transfer.n,
            s.transfer.us,
            s.transfer.max,
            s.lease.n,
            s.lease.us,
            s.lease.max
        );
        if let Some(t) = r.totals {
            log::info!(
                "native-exit ended={} cumulative submit={} done={} cancel={} busy={} slow_seen={} kept={}",
                r.exit_session,
                t.submitted,
                t.completed,
                t.cancelled,
                t.busy_us,
                r.slow_seen,
                r.slow_len
            );
            log::info!(
                "native-exit-time handler={}/{} flush={}/{} rpc={}/{} work={}/{}",
                t.handler.n,
                t.handler.us,
                t.flush.n,
                t.flush.us,
                t.rpc.n,
                t.rpc.us,
                t.work.n,
                t.work.us
            );
            for f in r.slow.iter().take(r.slow_len) {
                log::info!(
                    "native-frame id={} s={} h={}:{} lease={} submit={}",
                    f.id,
                    f.session,
                    f.handler_start,
                    f.handler_end,
                    f.leased,
                    f.submit,
                );
                log::info!(
                    "native-frame-plane id={} pickup={} te={} tx={} release={}",
                    f.id,
                    f.pickup,
                    f.te_end,
                    f.transfer_end,
                    f.release
                );
                log::info!(
                    "native-frame-draw id={} flush={}/{} px={} rect={},{},{},{}",
                    f.id,
                    f.flush_n,
                    f.flush_us,
                    f.flush_pixels,
                    f.rect[0],
                    f.rect[1],
                    f.rect[2],
                    f.rect[3]
                );
            }
        }
    }
}
#[cfg(target_os = "none")]
pub use device::{report, with};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn busy_time_splits_at_reports_and_idle_windows_are_zero() {
        let mut p = Profile::new();
        p.set_active(true, 100);
        p.begin_handler(200);
        p.flush(10, 100);
        p.end_handler(250);
        let id = p.lease(300, [0, 0, 9, 9]).unwrap();
        p.submit(id, 310);
        p.pickup(id, 320);
        let r = p.report(1_000_100).unwrap();
        assert_eq!(r.stats.busy_us, 999_800);
        assert_eq!(r.stats.completed, 0);
        assert_eq!(r.stats.flush_pixels, 100);
        p.te_end(id, 1_000_200);
        p.transfer_end(id, 1_000_300);
        p.release(id, 1_000_400);
        let r = p.report(2_000_100).unwrap();
        assert_eq!(r.stats.busy_us, 300);
        assert_eq!(r.stats.completed, 1);
        assert_eq!(r.stats.transfer.us, 100);
        let r = p.report(3_000_100).unwrap();
        assert_eq!(r.stats.completed, 0);
        assert_eq!(r.stats.handler.n, 0);
        assert_eq!(r.stats.busy_us, 0);
    }
    #[test]
    fn exit_waits_for_release_and_cancelled_frames_keep_their_identity() {
        let mut p = Profile::new();
        p.set_active(true, 1);
        let id = p.lease(10, [0, 0, 3, 3]).unwrap();
        p.submit(id, 11);
        p.set_active(false, 20);
        assert!(p.report(1_000_001).unwrap().totals.is_none());
        p.release(id + 1, 1_000_002);
        assert!(p.current.is_some());
        p.release(id, 1_000_003);
        let r = p.report(2_000_001).unwrap();
        let t = r.totals.unwrap();
        assert_eq!(t.cancelled, 1);
        assert_eq!(t.completed, 0);
        assert_eq!(t.busy_us, 999_993);
        assert!(p.report(3_000_001).is_none());
    }
    #[test]
    fn new_session_resets_idle_time_and_late_frames_mark_mixed_windows() {
        let mut p = Profile::new();
        p.set_active(true, 1);
        p.set_active(false, 2);
        assert!(p.report(1_000_001).unwrap().totals.is_some());
        p.set_active(true, 100_000_000);
        assert!(p.report(100_000_001).is_none());
        let id = p.lease(100_000_010, [0, 0, 3, 3]).unwrap();
        p.submit(id, 100_000_011);
        p.set_active(false, 100_000_020);
        p.set_active(true, 100_000_030);
        let r = p.report(101_000_000).unwrap();
        assert_eq!((r.first_session, r.session), (2, 3));
        assert_eq!(r.exit_session, 2);
        assert_eq!(r.stats.completed, 0);
        // Even a second window with no phase events retains the old lease's
        // session: its BUSY time still belongs to session 2.
        let r = p.report(102_000_000).unwrap();
        assert_eq!((r.first_session, r.session), (2, 3));
        assert_eq!(r.stats.busy_us, 1_000_000);
        p.pickup(id, 102_000_040);
        p.te_end(id, 102_000_050);
        p.transfer_end(id, 102_000_060);
        p.release(id, 102_000_070);
        let r = p.report(103_000_000).unwrap();
        assert_eq!((r.first_session, r.session), (2, 3));
        assert_eq!(r.stats.completed, 1);
        assert_eq!(r.at - r.window_start, 1_000_000);
    }

    #[test]
    fn counts_sums_maxima_and_handler_flushes_are_distinct() {
        let mut p = Profile::new();
        p.set_active(true, 1);
        p.loop_start(10);
        p.loop_start(30);
        p.loop_start(60);
        p.work(5);
        p.work(7);
        p.rpc(3);
        p.rpc(4);
        // A flush outside the handler contributes to aggregate work but must
        // not be assigned to a later handler/frame.
        p.flush(2, 8);
        p.begin_handler(100);
        p.flush(5, 10);
        p.flush(7, 20);
        p.end_handler(130);
        let id = p.lease(140, [1, 2, 3, 5]).unwrap();
        let f = p.current.unwrap();
        assert_eq!((f.flush_n, f.flush_us, f.flush_pixels), (2, 12, 30));
        p.submit(id, 150);
        p.pickup(id, 170);
        p.te_end(id, 200);
        p.transfer_end(id, 240);
        p.release(id, 260);
        p.busy_skip();
        let r = p.report(1_000_001).unwrap();
        assert_eq!(
            r.stats.loops,
            Metric {
                n: 2,
                us: 50,
                max: 30
            }
        );
        assert_eq!(
            r.stats.work,
            Metric {
                n: 2,
                us: 12,
                max: 7
            }
        );
        assert_eq!(
            r.stats.rpc,
            Metric {
                n: 2,
                us: 7,
                max: 4
            }
        );
        assert_eq!(
            r.stats.handler,
            Metric {
                n: 1,
                us: 30,
                max: 30
            }
        );
        assert_eq!(
            r.stats.flush,
            Metric {
                n: 3,
                us: 14,
                max: 7
            }
        );
        assert_eq!(r.stats.flush_pixels, 38);
        assert_eq!(r.stats.damage_pixels, 12);
        assert_eq!(r.stats.queue.us, 20);
        assert_eq!(r.stats.te.us, 30);
        assert_eq!(r.stats.transfer.us, 40);
        assert_eq!(r.stats.lease.us, 120);
        assert_eq!(r.stats.busy_skips, 1);
    }

    #[test]
    fn startup_cannot_exhaust_last_slow_frame_ring() {
        let mut p = Profile::new();
        p.set_active(true, 1);
        for i in 0..24u64 {
            let start = 1 + i * 100_000;
            p.begin_handler(start);
            p.end_handler(start + 10);
            let id = p.lease(start + 20, [0, 0, 3, 3]).unwrap();
            p.submit(id, start + 21);
            p.pickup(id, start + 22);
            p.te_end(id, start + 23);
            p.transfer_end(id, start + 60_000);
            p.release(id, start + 60_001);
        }
        p.set_active(false, 3_000_001);
        let r = p.report(3_000_001).unwrap();
        assert_eq!(r.slow_seen, 24);
        assert_eq!(r.slow_len, SLOW_CAPACITY);
        assert_eq!(r.slow[0].id, 13);
        assert_eq!(r.slow[11].id, 24);
        assert_eq!(r.totals.unwrap().completed, 24);
    }
}
