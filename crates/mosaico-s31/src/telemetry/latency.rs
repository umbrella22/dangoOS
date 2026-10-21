//! Touch-to-present latency instrumentation for the R1 device gate.
//!
//! # What is measured
//!
//! `ui_task` timestamps every press edge when it drains the input queue and
//! tags the next plane job with it. The tag travels in
//! `PlaneJob::touch_at_us`, so only a frame that the UI rendered *after*
//! applying the touch can carry it. [`note_first_pixel`] is called by
//! `plane_task` immediately before the first QSPI chunk of that tagged
//! job:
//!
//! ```text
//! GPIO6 IRQ -> input_task reads CST9220 -> INPUT channel
//!   -> ui_task drains + applies the press (Ui::drain)
//!   -> Ui::tick rasterizes the response into a canvas-pool slot
//!   -> PlaneUi::submit(tagged) -> plane_task
//!   -> note_first_pixel(at_us) -> first write_canvas_async()
//! ```
//!
//! This is **touch report read completion -> first QSPI byte**, not panel
//! photon latency: it excludes the CST9220 scan/IRQ-to-read delay and the
//! panel's own scan-out, and it includes the fox render, the plane
//! handoff and the first chunk's packing. Tagged jobs skip the TE wait. It is
//! the firmware-side deadline used by the P95 <= 50 ms gate.
//!
//! # Why the tag matters
//!
//! Earlier versions kept one global pending timestamp. A frame that had
//! already been submitted (for example an animation frame rendered before
//! the press) could consume it, producing samples that did not belong to the
//! response frame. The tag in the job makes the association exact. When a
//! newer job replaces an unsent one, the earliest pending tag moves to the
//! newer job, so a press is never lost to frame replacement.
//!
//! `at_us` comes from [`esp_hal::time::Instant`] in `input_task`, and
//! [`now_us`] uses the same monotonic clock.
//!
//! # Storage
//!
//! Samples live in a fixed ring of [`CAPACITY`] milliseconds; no allocation is
//! performed. The ring is protected by a `critical_section` mutex, which is
//! enough because both producers run on the single executor thread.

#[cfg(target_os = "none")]
use core::cell::RefCell;

/// Number of samples retained by the ring (the most recent touches).
pub const CAPACITY: usize = 100;

/// Pure fixed-capacity millisecond sample ring (host-testable).
///
/// `record` overwrites the oldest sample once the ring is full; `summary`
/// sorts a stack copy and computes nearest-rank P50/P95 plus the maximum.
pub struct SampleRing {
    samples: [u32; CAPACITY],
    len: usize,
    next: usize,
}

impl SampleRing {
    pub const fn new() -> Self {
        Self {
            samples: [0; CAPACITY],
            len: 0,
            next: 0,
        }
    }

    /// Add one millisecond sample, evicting the oldest when full.
    pub fn record(&mut self, millis: u32) {
        self.samples[self.next] = millis;
        self.next = (self.next + 1) % CAPACITY;
        if self.len < CAPACITY {
            self.len += 1;
        }
    }

    /// Number of live samples (`<= CAPACITY`).
    #[allow(dead_code)] // Used by host tests and diagnostic callers.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// `(count, p50, p95, max)`, all in milliseconds. Zeroes when empty.
    ///
    /// P50/P95 use the nearest-rank definition: index
    /// `ceil(percent * count / 100) - 1` in ascending order.
    pub fn summary(&self) -> (u32, u32, u32, u32) {
        if self.len == 0 {
            return (0, 0, 0, 0);
        }
        let mut sorted = [0u32; CAPACITY];
        if self.len == CAPACITY {
            // Ring order starts at `next` once full.
            sorted[..self.len].copy_from_slice(&self.samples);
            sorted[..self.len].rotate_left(self.next);
        } else {
            sorted[..self.len].copy_from_slice(&self.samples[..self.len]);
        }
        let values = &mut sorted[..self.len];
        // Insertion sort: bounded by CAPACITY, allocated on the stack.
        for index in 1..values.len() {
            let value = values[index];
            let mut hole = index;
            while hole > 0 && values[hole - 1] > value {
                values[hole] = values[hole - 1];
                hole -= 1;
            }
            values[hole] = value;
        }
        let count = self.len as u32;
        (
            count,
            percentile(values, 50),
            percentile(values, 95),
            values[values.len() - 1],
        )
    }
}

impl Default for SampleRing {
    fn default() -> Self {
        Self::new()
    }
}

fn percentile(sorted: &[u32], percent: usize) -> u32 {
    // Nearest rank: ceil(percent * n / 100) - 1.
    let rank = (percent * sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// Monotonic microseconds since boot (same clock as `input_task::at_us`).
#[cfg(target_os = "none")]
pub fn now_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

#[cfg(target_os = "none")]
static SAMPLES: critical_section::Mutex<RefCell<SampleRing>> =
    critical_section::Mutex::new(RefCell::new(SampleRing::new()));

/// Convert the timestamp carried by a tagged frame into one sample.
///
/// Called by `plane_task` immediately before the first QSPI chunk of a job
/// whose `PlaneJob::touch_at_us` is set. The stored value is
/// the elapsed microseconds divided by 1000 (saturating at `u32::MAX`).
#[cfg(target_os = "none")]
pub fn note_first_pixel(at_us: u64) {
    let elapsed_us = now_us().saturating_sub(at_us);
    let millis = (elapsed_us / 1000).min(u32::MAX as u64) as u32;
    critical_section::with(|cs| SAMPLES.borrow_ref_mut(cs).record(millis));
}

/// `(count, p50, p95, max)` in milliseconds.
#[cfg(target_os = "none")]
pub fn summary() -> (u32, u32, u32, u32) {
    critical_section::with(|cs| SAMPLES.borrow_ref(cs).summary())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ring_reports_zeroes() {
        let ring = SampleRing::new();
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.summary(), (0, 0, 0, 0));
    }

    #[test]
    fn nearest_rank_percentiles() {
        let mut ring = SampleRing::new();
        for value in 1..=100 {
            ring.record(value);
        }
        assert_eq!(ring.summary(), (100, 50, 95, 100));
    }

    #[test]
    fn ring_keeps_the_newest_samples() {
        let mut ring = SampleRing::new();
        for value in 0..CAPACITY as u32 {
            ring.record(value);
        }
        // Overwrite everything with 7..=106; the oldest 0..6 are evicted.
        for value in 7..=106 {
            ring.record(value);
        }
        assert_eq!(ring.len(), CAPACITY);
        assert_eq!(ring.summary(), (100, 56, 101, 106));
    }

    #[test]
    fn small_and_duplicate_samples() {
        let mut ring = SampleRing::new();
        ring.record(9);
        assert_eq!(ring.summary(), (1, 9, 9, 9));
        ring.record(3);
        ring.record(3);
        ring.record(12);
        // Sorted: 3, 3, 9, 12 -> p50 rank 2 -> 3, p95 rank 4 -> 12.
        assert_eq!(ring.summary(), (4, 3, 12, 12));
    }
}
