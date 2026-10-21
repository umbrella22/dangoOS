//! Two-slot canvas pool: typed ownership of the ball canvas between the
//! renderer and the panel upload.
//!
//! Before this module the widget owned one canvas, handed a `&'static [u16]`
//! to the plane task and relied on a `DISPLAY_ACTIVE` flag (check, then store)
//! to keep the rasterizer away from a buffer the DMA was still reading. That
//! was a lifetime promise in a comment, and it serialized raster and upload.
//!
//! Here each slot is in exactly one state at a time:
//!
//! ```text
//!   FREE ──acquire──▶ WRITING ──publish──▶ READY ──take_ready──▶ READING
//!    ▲                   │                   │                      │
//!    └──── drop ─────────┘  (re)acquire ◀────┘ (newest wins)        │
//!    └──────────────────────────── drop ────────────────────────────┘
//! ```
//!
//! * [`WriteLease`] is the only way to get `&mut [u16]`; it exists only in
//!   `WRITING`.
//! * [`ReadLease`] is the only way to get `&[u16]` for the upload; it exists
//!   only in `READING` and returns the slot to `FREE` on drop.
//! * Every transition is a compare-exchange, so the renderer (thread mode) and
//!   the upload task (interrupt executor) can never both own a slot.
//!
//! With two slots the renderer draws frame N+1 while frame N is on the wire.
//! If the upload is slower than the renderer, a `READY` frame that was never
//! taken is recycled: the panel always receives the newest picture.
//!
//! The pool is `'static` (its buffers are leaked once at boot) because the ball
//! canvas lives for the whole firmware run; `'static` here is a fact, not a
//! comment.

use alloc::boxed::Box;
use alloc::vec;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

const FREE: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;
const READING: u8 = 3;

/// Number of canvas slots.
pub const SLOTS: usize = 2;

struct Slot {
    state: AtomicU8,
    /// Publication sequence number; larger is newer.
    seq: AtomicU32,
    ptr: *mut u16,
    len: usize,
}

/// Fixed pool of [`SLOTS`] RGB565 canvases.
pub struct CanvasPool {
    slots: [Slot; SLOTS],
    next_seq: AtomicU32,
    /// Frames recycled before the upload took them.
    dropped: AtomicU32,
}

// SAFETY: the raw pointers are only dereferenced through a lease, and a lease
// exists only after a successful compare-exchange into an exclusive state.
unsafe impl Sync for CanvasPool {}
// SAFETY: see above; the buffers are leaked heap memory, not thread-local.
unsafe impl Send for CanvasPool {}

impl CanvasPool {
    /// Allocate the pool and its buffers once and leak them.
    pub fn leak(pixels: usize) -> &'static CanvasPool {
        let make = || {
            let buffer: &'static mut [u16] = Box::leak(vec![0u16; pixels].into_boxed_slice());
            Slot {
                state: AtomicU8::new(FREE),
                seq: AtomicU32::new(0),
                ptr: buffer.as_mut_ptr(),
                len: buffer.len(),
            }
        };
        Box::leak(Box::new(CanvasPool {
            slots: [make(), make()],
            next_seq: AtomicU32::new(1),
            dropped: AtomicU32::new(0),
        }))
    }

    /// Pixels per canvas.
    pub fn pixels(&self) -> usize {
        self.slots[0].len
    }

    /// Frames recycled while still `READY` (the upload fell behind).
    pub fn dropped_frames(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Take a slot for rendering.
    ///
    /// Prefers a `FREE` slot; otherwise recycles a `READY` frame the upload has
    /// not taken yet. Returns `None` only while one slot is being written and
    /// the other is on the wire.
    pub fn acquire(&'static self) -> Option<WriteLease> {
        for (index, slot) in self.slots.iter().enumerate() {
            if transition(slot, FREE, WRITING) {
                return Some(WriteLease { pool: self, index });
            }
        }
        for (index, slot) in self.slots.iter().enumerate() {
            if transition(slot, READY, WRITING) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return Some(WriteLease { pool: self, index });
            }
        }
        None
    }

    /// Take the newest published frame for reading (upload or LVGL display).
    pub fn take_ready(&'static self) -> Option<ReadLease> {
        let mut best: Option<(usize, u32)> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.state.load(Ordering::Acquire) == READY {
                let seq = slot.seq.load(Ordering::Relaxed);
                if best.map_or(true, |(_, s)| seq.wrapping_sub(s) as i32 > 0) {
                    best = Some((index, seq));
                }
            }
        }
        let (index, _) = best?;
        if transition(&self.slots[index], READY, READING) {
            Some(ReadLease { pool: self, index })
        } else {
            // The renderer recycled it between the scan and the exchange.
            None
        }
    }

    #[cfg(test)]
    fn state(&self, index: usize) -> u8 {
        self.slots[index].state.load(Ordering::Acquire)
    }
}

fn transition(slot: &Slot, from: u8, to: u8) -> bool {
    slot.state
        .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Exclusive write access to one canvas.
pub struct WriteLease {
    pool: &'static CanvasPool,
    index: usize,
}

impl WriteLease {
    /// The canvas pixels. Contents are whatever the slot last held.
    pub fn pixels_mut(&mut self) -> &mut [u16] {
        let slot = &self.pool.slots[self.index];
        // SAFETY: this lease is the unique owner of the slot (state WRITING).
        unsafe { core::slice::from_raw_parts_mut(slot.ptr, slot.len) }
    }

    /// Publish the frame as the newest `READY` canvas.
    pub fn publish(self) {
        let slot = &self.pool.slots[self.index];
        let seq = self.pool.next_seq.fetch_add(1, Ordering::Relaxed);
        slot.seq.store(seq, Ordering::Relaxed);
        slot.state.store(READY, Ordering::Release);
        core::mem::forget(self);
    }
}

impl Drop for WriteLease {
    /// An abandoned frame goes straight back to `FREE`.
    fn drop(&mut self) {
        self.pool.slots[self.index]
            .state
            .store(FREE, Ordering::Release);
    }
}

/// Shared read access to one published canvas.
pub struct ReadLease {
    pool: &'static CanvasPool,
    index: usize,
}

impl ReadLease {
    /// The canvas pixels.
    pub fn pixels(&self) -> &[u16] {
        let slot = &self.pool.slots[self.index];
        // SAFETY: the slot is READING; no write lease can exist for it.
        unsafe { core::slice::from_raw_parts(slot.ptr, slot.len) }
    }

    /// Raw address for LVGL's canvas descriptor (host preview path).
    pub fn as_ptr(&self) -> *const u16 {
        self.pool.slots[self.index].ptr
    }
}

impl Drop for ReadLease {
    fn drop(&mut self) {
        self.pool.slots[self.index]
            .state
            .store(FREE, Ordering::Release);
    }
}

// SAFETY: a lease is a unique token for one slot of a `Sync` pool; moving it to
// the upload task (interrupt executor) moves that ownership with it.
unsafe impl Send for ReadLease {}
unsafe impl Send for WriteLease {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> &'static CanvasPool {
        CanvasPool::leak(16)
    }

    #[test]
    fn a_published_frame_is_read_back() {
        let pool = pool();
        let mut write = pool.acquire().expect("free slot");
        write.pixels_mut().fill(0xABCD);
        write.publish();
        let read = pool.take_ready().expect("ready frame");
        assert!(read.pixels().iter().all(|&p| p == 0xABCD));
    }

    #[test]
    fn rendering_overlaps_the_upload() {
        let pool = pool();
        pool.acquire().unwrap().publish();
        let on_wire = pool.take_ready().unwrap();
        // While frame N is uploading, frame N+1 gets the other slot.
        let next = pool.acquire().expect("second slot is free during upload");
        assert_ne!(next.index, on_wire.index);
        drop(next);
        drop(on_wire);
        assert_eq!(pool.state(0), FREE);
        assert_eq!(pool.state(1), FREE);
    }

    #[test]
    fn the_uploader_never_sees_a_slot_being_written() {
        let pool = pool();
        let _writing = pool.acquire().unwrap();
        assert!(pool.take_ready().is_none());
    }

    #[test]
    fn a_stale_ready_frame_is_recycled_and_the_newest_wins() {
        let pool = pool();
        let on_wire = {
            pool.acquire().unwrap().publish();
            pool.take_ready().unwrap()
        };
        // Upload is slow: two frames are rendered meanwhile.
        let mut a = pool.acquire().unwrap();
        a.pixels_mut().fill(1);
        a.publish();
        let mut b = pool.acquire().expect("recycles the untaken READY frame");
        b.pixels_mut().fill(2);
        b.publish();
        assert_eq!(pool.dropped_frames(), 1);
        drop(on_wire);
        let read = pool.take_ready().unwrap();
        assert!(read.pixels().iter().all(|&p| p == 2));
    }

    #[test]
    fn both_slots_busy_means_no_lease() {
        let pool = pool();
        pool.acquire().unwrap().publish();
        let _on_wire = pool.take_ready().unwrap();
        let _writing = pool.acquire().unwrap();
        assert!(pool.acquire().is_none());
    }

    #[test]
    fn newest_of_two_ready_frames_is_taken() {
        let pool = pool();
        let mut a = pool.acquire().unwrap();
        let mut b = pool.acquire().unwrap();
        a.pixels_mut().fill(1);
        b.pixels_mut().fill(2);
        a.publish();
        b.publish();
        let read = pool.take_ready().unwrap();
        assert!(read.pixels().iter().all(|&p| p == 2));
    }
}
