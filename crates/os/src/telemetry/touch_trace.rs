//! Optional bounded phase receipts. The original latency ring remains authoritative.
//! Timestamps and durations are microseconds; no measurement point logs or allocates.

const CAPACITY: usize = 100;

#[derive(Clone, Copy, Default)]
pub struct Raster {
    pub start: u64,
    pub end: u64,
    pub loop_id: u32,
    pub due: bool,
    pub urgent: bool,
}

#[derive(Clone, Copy, Default)]
pub struct Record {
    pub seq: u32,
    pub touch: u64,
    pub dequeue: u64,
    pub handled: u64,
    pub raster: Raster,
    pub submit: u64,
    pub pickup: u64,
    pub finish: u64,
    pub input_loop: u32,
    pub sensor_us: u64,
    pub rpc_before: (u64, u64),
    pub rpc: (u64, u64),
    pub pool_misses: u32,
    pub cadence_skips: u32,
    emitted: bool,
}

pub struct Trace {
    rows: [Record; CAPACITY],
    pub completed: u32,
    pub emitted: u32,
    pub merged: u32,
    next_seq: u32,
    loop_id: u32,
    sensor_us: u64,
    rpc: (u64, u64),
    raster: Raster,
}

impl Trace {
    pub const fn new() -> Self {
        const EMPTY: Record = Record {
            seq: 0,
            touch: 0,
            dequeue: 0,
            handled: 0,
            raster: Raster {
                start: 0,
                end: 0,
                loop_id: 0,
                due: false,
                urgent: false,
            },
            submit: 0,
            pickup: 0,
            finish: 0,
            input_loop: 0,
            sensor_us: 0,
            rpc_before: (0, 0),
            rpc: (0, 0),
            pool_misses: 0,
            cadence_skips: 0,
            emitted: false,
        };
        Self {
            rows: [EMPTY; CAPACITY],
            completed: 0,
            emitted: 0,
            merged: 0,
            next_seq: 0,
            loop_id: 0,
            sensor_us: 0,
            rpc: (0, 0),
            raster: EMPTY.raster,
        }
    }

    fn row(&mut self, tag: u64) -> Option<&mut Record> {
        self.rows.iter_mut().find(|r| r.seq != 0 && r.touch == tag)
    }
    pub fn begin_loop(&mut self) {
        self.loop_id = self.loop_id.wrapping_add(1);
    }
    pub fn sensor(&mut self, us: u64) {
        self.sensor_us = us;
    }
    pub fn rpc(&mut self, us: u64) {
        self.rpc.0 += 1;
        self.rpc.1 += us;
    }
    pub fn rpc_snapshot(&self) -> (u64, u64) {
        self.rpc
    }
    pub fn accept(&mut self, tag: u64, dequeue: u64, handled: u64, before: (u64, u64)) {
        if self.completed >= CAPACITY as u32 || self.row(tag).is_some() {
            return;
        }
        if let Some(r) = self.rows.iter_mut().find(|r| r.seq == 0) {
            self.next_seq += 1;
            *r = Record {
                seq: self.next_seq,
                touch: tag,
                dequeue,
                handled,
                input_loop: self.loop_id,
                sensor_us: self.sensor_us,
                rpc_before: before,
                ..Record::default()
            };
        }
    }
    pub fn pool_miss(&mut self) {
        for r in &mut self.rows {
            if r.seq != 0 && r.submit == 0 && r.finish == 0 {
                r.pool_misses += 1;
            }
        }
    }
    pub fn cadence_skip(&mut self) {
        for r in &mut self.rows {
            if r.seq != 0 && r.submit == 0 && r.finish == 0 {
                r.cadence_skips += 1;
            }
        }
    }
    pub fn raster(&mut self, start: u64, end: u64, due: bool, urgent: bool) {
        self.raster = Raster {
            start,
            end,
            due,
            urgent,
            loop_id: self.loop_id,
        };
    }
    /// Called after normal earliest-tag inheritance. Superseded input tags no
    /// longer represent separate latency samples, so release those records.
    pub fn submit(
        &mut self,
        original: Option<u64>,
        stale: Option<u64>,
        inherited: Option<u64>,
        at: u64,
    ) {
        for discarded in [original, stale].into_iter().flatten() {
            if Some(discarded) != inherited {
                if let Some(r) = self.row(discarded) {
                    *r = Record::default();
                    self.merged += 1;
                }
            }
        }
        let raster = self.raster;
        let rpc = self.rpc;
        if let Some(r) = inherited.and_then(|tag| self.row(tag)) {
            r.raster = raster;
            r.submit = at;
            // Includes pointer RPC and intervening sensor/tick RPCs through submit.
            r.rpc = (rpc.0 - r.rpc_before.0, rpc.1 - r.rpc_before.1);
        }
    }
    pub fn pickup(&mut self, tag: u64, at: u64) {
        if let Some(r) = self.row(tag) {
            r.pickup = at;
        }
    }
    pub fn finish(&mut self, tag: u64, at: u64) {
        if let Some(r) = self.row(tag) {
            if r.finish == 0 {
                r.finish = at;
                self.completed += 1;
            }
        }
    }
    /// Copy one completed row; formatting/CDC I/O happen outside the lock.
    pub fn take_completed(&mut self) -> Option<Record> {
        let r = self.rows.iter_mut().find(|r| r.finish != 0 && !r.emitted)?;
        r.emitted = true;
        self.emitted += 1;
        Some(*r)
    }
}

#[cfg(target_os = "none")]
mod device {
    use super::*;
    use core::cell::RefCell;
    static TRACE: critical_section::Mutex<RefCell<Trace>> =
        critical_section::Mutex::new(RefCell::new(Trace::new()));
    pub fn with<R>(f: impl FnOnce(&mut Trace) -> R) -> R {
        critical_section::with(|cs| f(&mut TRACE.borrow_ref_mut(cs)))
    }
    pub fn now() -> u64 {
        super::super::latency::now_us()
    }
    /// Drain at most four completed rows at the existing one-second stats point.
    /// Each line stays below the console's 160-byte capacity at device-session uptimes. seq and final
    /// completed/emitted counts expose dropped log lines and pending output.
    pub fn report() {
        for _ in 0..4 {
            let Some(r) = with(Trace::take_completed) else {
                break;
            };
            log::info!(
                "tt s={} a t={} dq={} h={} rs={} re={}",
                r.seq,
                r.touch,
                r.dequeue,
                r.handled,
                r.raster.start,
                r.raster.end
            );
            log::info!(
                "tt s={} b sub={} pick={} end={} loops={}/{} due={} urg={} miss={} skips={}",
                r.seq,
                r.submit,
                r.pickup,
                r.finish,
                r.input_loop,
                r.raster.loop_id,
                r.raster.due as u8,
                r.raster.urgent as u8,
                r.pool_misses,
                r.cadence_skips
            );
            log::info!(
                "tt s={} c sensor={} rpc={}/{} total={}",
                r.seq,
                r.sensor_us,
                r.rpc.0,
                r.rpc.1,
                r.finish.saturating_sub(r.touch)
            );
        }
        let (done, sent, merged) = with(|t| (t.completed, t.emitted, t.merged));
        log::info!("tt counts completed={done} emitted={sent} merged={merged} cap=100");
    }
}
#[cfg(target_os = "none")]
pub use device::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_preserves_old_origin_and_uses_new_raster() {
        let mut t = Trace::new();
        t.begin_loop();
        t.sensor(7);
        t.accept(10, 12, 14, (0, 0));
        t.raster(15, 20, false, true);
        t.submit(Some(10), None, Some(10), 21);
        t.begin_loop();
        t.accept(30, 32, 34, (0, 0));
        t.rpc(9);
        t.raster(35, 40, true, false);
        t.submit(Some(30), Some(10), Some(10), 41);
        t.pickup(10, 42);
        t.finish(10, 43);
        let r = t.take_completed().unwrap();
        assert_eq!(
            (
                r.touch,
                r.dequeue,
                r.raster.start,
                r.submit,
                r.pickup,
                r.finish
            ),
            (10, 12, 35, 41, 42, 43)
        );
        assert_eq!(r.rpc, (1, 9));
        assert_eq!(t.merged, 1);
        assert!(t.take_completed().is_none());
        t.finish(10, 44);
        assert_eq!(t.completed, 1);
    }
    #[test]
    fn capped_at_one_hundred_completed_samples() {
        let mut t = Trace::new();
        for tag in 1..=105 {
            t.accept(tag, tag, tag, (0, 0));
            t.finish(tag, tag + 1);
        }
        assert_eq!(t.completed, 100);
        assert_eq!((0..105).filter_map(|_| t.take_completed()).count(), 100);
        assert_eq!(t.emitted, 100);
    }
}
