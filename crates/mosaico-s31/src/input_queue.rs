//! Samples collected between LVGL calls. Adjacent motion and buffered pressed
//! heartbeats coalesce without moving the original press coordinate/timestamp.
//! A press sequence retains its latest pressed sampling time for replay before
//! release, even when stationary or its press edge was already delivered. An
//! empty queue may mean rendering has blocked dispatch, not that manual reads
//! are advancing the held state's time. Duplicate releases are always ignored.
//! Overflow evicts the oldest queued non-edge (motion or heartbeat) first.
//! An all-edge queue rejects incoming non-edges; a new edge drops the oldest
//! edge. Gesture history may be lost in that overload, but delivery converges
//! to the newest pressed/released state. Edge delivery is not lossless.

pub const CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    sample: Sample,
    sampled_at_us: u64,
    edge: bool,
}

pub struct InputQueue {
    entries: [Entry; CAPACITY],
    head: usize,
    len: usize,
    observed: Sample,
    delivered: Sample,
    pub overflows: u32,
}

impl InputQueue {
    pub fn new() -> Self {
        Self {
            entries: [Entry::default(); CAPACITY],
            head: 0,
            len: 0,
            observed: Sample::default(),
            delivered: Sample::default(),
            overflows: 0,
        }
    }

    pub fn observed(&self) -> Sample {
        self.observed
    }

    pub fn is_pending(&self) -> bool {
        self.len != 0
    }

    pub fn push(&mut self, sample: Sample, sampled_at_us: u64) {
        // Keep one latest pressed read before release, even after the press
        // edge was drained: rendering may currently be blocking dispatch, so
        // replay must still advance LVGL's long-press clock before releasing.
        if sample == self.observed && !sample.pressed {
            return;
        }
        let edge = sample.pressed != self.observed.pressed;
        self.observed = sample;
        // Preserve the position/time of a press edge. Coalesce both motion
        // and stationary heartbeats only into a following non-edge entry.
        if !edge && self.len > 0 {
            let tail = (self.head + self.len - 1) % CAPACITY;
            if !self.entries[tail].edge {
                self.entries[tail] = Entry {
                    sample,
                    sampled_at_us,
                    edge,
                };
                return;
            }
        }
        if self.len == CAPACITY {
            self.overflows = self.overflows.saturating_add(1);
            if let Some(offset) =
                (0..self.len).find(|&i| !self.entries[(self.head + i) % CAPACITY].edge)
            {
                // Remove the oldest non-edge, keeping all remaining entries in
                // FIFO order, including press coordinates and timestamps. Shift
                // toward the hole in logical ring order, even across the wrap.
                // Both the scan and shift are bounded by CAPACITY.
                for i in offset..self.len - 1 {
                    let dst = (self.head + i) % CAPACITY;
                    let src = (self.head + i + 1) % CAPACITY;
                    self.entries[dst] = self.entries[src];
                }
            } else if !edge {
                // Do not sacrifice a press/release for incoming motion or a
                // heartbeat. A later held sample can retry when space is free.
                return;
            } else {
                // All-edge overload cannot be lossless. Drop the oldest edge;
                // gesture history may be lost, but retain the newest state so
                // delivery still converges to hardware (including its release).
                self.head = (self.head + 1) % CAPACITY;
            }
            self.len -= 1;
        }
        let tail = (self.head + self.len) % CAPACITY;
        self.entries[tail] = Entry {
            sample,
            sampled_at_us,
            edge,
        };
        self.len += 1;
    }

    /// Queued samples, including buffered heartbeats, carry their sampling
    /// timestamp. An empty-queue repeat of the delivered state has no timestamp.
    pub fn read(&mut self) -> (Sample, Option<u64>) {
        if self.len == 0 {
            return (self.delivered, None);
        }
        let entry = self.entries[self.head];
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        self.delivered = entry.sample;
        (entry.sample, Some(entry.sampled_at_us))
    }
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(x: i32, pressed: bool) -> Sample {
        Sample { x, y: 20, pressed }
    }

    #[test]
    fn short_tap_preserves_both_edges() {
        let mut q = InputQueue::new();
        q.push(sample(10, true), 5);
        q.push(sample(10, false), 9);
        assert_eq!(q.read(), (sample(10, true), Some(5)));
        assert_eq!(q.read(), (sample(10, false), Some(9)));
        assert_eq!(q.read(), (sample(10, false), None));
    }

    #[test]
    fn motion_coalesces_without_moving_press_origin() {
        let mut q = InputQueue::new();
        q.push(sample(10, true), 1);
        q.push(sample(11, true), 2);
        q.push(sample(30, true), 3);
        q.push(sample(30, true), 4); // Heartbeat updates the motion's timestamp.
        q.push(sample(30, false), 5);
        assert_eq!(q.read(), (sample(10, true), Some(1)));
        assert_eq!(q.read(), (sample(30, true), Some(4)));
        assert_eq!(q.read(), (sample(30, false), Some(5)));
        assert!(!q.is_pending());
    }

    #[test]
    fn duplicate_reports_do_not_fill_queue() {
        let mut q = InputQueue::new();
        for t in 0..100 {
            q.push(sample(10, true), t);
        }
        // Only the original Down and one latest heartbeat are retained.
        assert_eq!(q.read(), (sample(10, true), Some(0)));
        assert_eq!(q.read(), (sample(10, true), Some(99)));
        assert!(!q.is_pending());
        assert_eq!(q.overflows, 0);
    }

    #[test]
    fn buffered_stationary_hold_retains_timestamps_before_release() {
        let mut q = InputQueue::new();
        let down = sample(10, true);
        let up = sample(10, false);
        q.push(down, 10_000);
        for t in [20_000, 300_000, 700_000] {
            q.push(down, t);
        }
        q.push(up, 750_000);
        q.push(up, 760_000); // Duplicate release must not change its timestamp.
        assert_eq!(q.read(), (down, Some(10_000)));
        // Native LVGL must see this late pressed read before the release to
        // recognize a long press rather than replaying only Down/Up as a click.
        assert_eq!(q.read(), (down, Some(700_000)));
        assert_eq!(q.read(), (up, Some(750_000)));
        assert!(!q.is_pending());
        assert_eq!(q.read(), (up, None));
        assert_eq!(q.overflows, 0);
    }

    #[test]
    fn pressed_reports_after_drain_keep_one_heartbeat_but_releases_do_not() {
        let mut q = InputQueue::new();
        let down = sample(10, true);
        let up = sample(10, false);
        q.push(down, 5);
        assert_eq!(q.read(), (down, Some(5)));
        for t in 6..100 {
            q.push(down, t);
        }
        assert_eq!(q.read(), (down, Some(99)));
        assert!(!q.is_pending());
        assert_eq!(q.read(), (down, None));

        q.push(up, 100);
        for t in 101..200 {
            q.push(up, t);
        }
        assert_eq!(q.read(), (up, Some(100)));
        assert!(!q.is_pending());
        for t in 200..300 {
            q.push(up, t);
        }
        assert!(!q.is_pending());
        assert_eq!(q.read(), (up, None));
        assert_eq!(q.overflows, 0);
    }

    #[test]
    fn overflow_is_bounded_and_final_release_is_delivered() {
        let mut q = InputQueue::new();
        for t in 0..40 {
            q.push(sample(10, t % 2 == 0), t);
        }
        let mut count = 0;
        let mut last = Sample::default();
        while q.is_pending() {
            last = q.read().0;
            count += 1;
        }
        assert_eq!(count, CAPACITY);
        assert_eq!(q.overflows, 24);
        assert!(!last.pressed);
    }

    #[test]
    fn full_mixed_queue_preserves_press_origin_and_evicts_oldest_motion() {
        // Exercise every physical head, including a wrapped interior removal.
        for head in 0..CAPACITY {
            let mut q = InputQueue::new();
            for i in 0..head {
                let released = sample(100 + i as i32, false);
                q.push(released, i as u64);
                assert_eq!(q.read(), (released, Some(i as u64)));
            }

            let mut burst = [(Sample::default(), 0u64); CAPACITY];
            for (i, entry) in burst.iter_mut().enumerate() {
                let s = match i {
                    0 => sample(10, true), // Original press must survive.
                    1 => sample(30, true), // Oldest motion: evict this one.
                    2 => sample(30, false),
                    3 => sample(40, true),
                    4 => sample(50, true), // Later motion must survive.
                    5 => sample(50, false),
                    _ => sample(60 + i as i32, i % 2 == 0),
                };
                *entry = (s, 100 + i as u64 * 10);
                q.push(entry.0, entry.1);
            }
            assert_eq!(q.overflows, 0);
            let newest = sample(90, true);
            q.push(newest, 300);
            assert_eq!(q.overflows, 1);

            // Check the original press's coordinate/time explicitly, then every
            // remaining sample/time in order. Only the oldest motion is lost.
            assert_eq!(q.read(), (sample(10, true), Some(100)));
            for &(s, timestamp) in &burst[2..] {
                assert_eq!(q.read(), (s, Some(timestamp)));
            }
            assert_eq!(q.read(), (newest, Some(300)));
            assert!(!q.is_pending());
            assert_eq!(q.read(), (newest, None));
        }
    }

    #[test]
    fn overflow_removes_motion_at_either_ring_end() {
        for motion_offset in [0, CAPACITY - 1] {
            for head in 0..CAPACITY {
                let mut q = InputQueue::new();
                for i in 0..head {
                    let released = sample(100 + i as i32, false);
                    q.push(released, i as u64);
                    assert_eq!(q.read(), (released, Some(i as u64)));
                }
                let mut burst = [(Sample::default(), 0u64); CAPACITY];
                let mut pressed = false;
                for (i, entry) in burst.iter_mut().enumerate() {
                    if i != motion_offset {
                        pressed = !pressed;
                    }
                    *entry = (sample(10 + i as i32, pressed), 100 + i as u64);
                    q.push(entry.0, entry.1);
                }
                let newest = sample(90, !pressed);
                q.push(newest, 200);
                assert_eq!(q.overflows, 1);
                for (i, &(s, timestamp)) in burst.iter().enumerate() {
                    if i != motion_offset {
                        assert_eq!(q.read(), (s, Some(timestamp)));
                    }
                }
                assert_eq!(q.read(), (newest, Some(200)));
                assert!(!q.is_pending());
            }
        }
    }

    #[test]
    fn incoming_non_edge_cannot_evict_an_all_edge_queue() {
        for x in [10, 50] {
            let mut q = InputQueue::new();
            let down = sample(10, true);
            q.push(down, 1);
            assert_eq!(q.read(), (down, Some(1)));

            // Start with a delivered press so the full queue ends pressed.
            for i in 0..CAPACITY {
                q.push(sample(10, i % 2 != 0), 100 + i as u64);
            }
            let latest = sample(x, true); // Either a heartbeat or motion.
            q.push(latest, 200);
            assert_eq!(q.overflows, 1);
            assert_eq!(q.observed(), latest);
            for i in 0..CAPACITY {
                assert_eq!(q.read(), (sample(10, i % 2 != 0), Some(100 + i as u64)));
            }
            assert!(!q.is_pending());

            // After space is available, a new sample can deliver the latest
            // held coordinate/time even if it matches the last observed state.
            q.push(latest, 201);
            q.push(sample(x, false), 202);
            assert_eq!(q.read(), (latest, Some(201)));
            assert_eq!(q.read(), (sample(x, false), Some(202)));
            assert!(!q.is_pending());
        }
    }

    #[test]
    fn queue_wrap_preserves_order() {
        let mut q = InputQueue::new();
        for t in 0..100 {
            let s = sample(t, t % 2 == 0);
            q.push(s, t as u64);
            assert_eq!(q.read(), (s, Some(t as u64)));
        }
    }
}
