//! Two-slot canvas pool: typed ownership of the fox canvas between the
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
//! The pool is `'static` (its buffers are leaked once at boot) because the fox
//! canvas lives for the whole firmware run; `'static` here is a fact, not a
//! comment.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use crate::platform::runtime::memory::PixelMemory;
use board_s31::pixels::{PixelFormat, PixelSource};

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
    /// Encoding of the published words; protected by the same READY/READING
    /// ownership boundary as their pixels. Native words remain the default.
    big_endian: AtomicBool,
    ptr: *mut u16,
    len: usize,
}

/// Fixed pool of [`SLOTS`] RGB565 canvases.
pub struct CanvasPool {
    slots: [Slot; SLOTS],
    next_seq: AtomicU32,
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
            // Direct PSRAM DMA needs cache-aligned storage. A Vec<u16> only
            // promises two-byte alignment; PixelMemory guarantees 128 bytes.
            let buffer = PixelMemory::new(pixels * core::mem::size_of::<u16>(), true);
            let ptr = buffer.as_ptr().cast::<u16>();
            let len = buffer.len() / core::mem::size_of::<u16>();
            // The pool and both buffers live for the whole firmware run.
            core::mem::forget(buffer);
            Slot {
                state: AtomicU8::new(FREE),
                seq: AtomicU32::new(0),
                big_endian: AtomicBool::new(false),
                ptr,
                len,
            }
        };
        Box::leak(Box::new(CanvasPool {
            slots: [make(), make()],
            next_seq: AtomicU32::new(1),
        }))
    }

    /// Pixels per canvas.
    pub fn pixels(&self) -> usize {
        self.slots[0].len
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
    /// The canvas storage words. Contents are whatever the slot last held.
    /// Before publication, write the complete frame in the selected encoding.
    pub fn pixels_mut(&mut self) -> &mut [u16] {
        let slot = &self.pool.slots[self.index];
        // SAFETY: this lease is the unique owner of the slot (state WRITING).
        unsafe { core::slice::from_raw_parts_mut(slot.ptr, slot.len) }
    }

    /// Publish the frame as the newest `READY` canvas.
    pub fn publish(self) {
        self.publish_encoded(false);
    }

    /// Publish words whose in-memory bytes are RGB565 big-endian, suitable for
    /// direct panel DMA. Every color store must already use `u16::to_be`.
    pub fn publish_rgb565be(self) {
        self.publish_encoded(true);
    }

    fn publish_encoded(self, big_endian: bool) {
        let slot = &self.pool.slots[self.index];
        let seq = self.pool.next_seq.fetch_add(1, Ordering::Relaxed);
        slot.seq.store(seq, Ordering::Relaxed);
        slot.big_endian.store(big_endian, Ordering::Relaxed);
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
    /// Logical RGB565 colors from a native publication, for LVGL/previews.
    pub fn pixels(&self) -> &[u16] {
        assert!(!self.pool.slots[self.index]
            .big_endian
            .load(Ordering::Relaxed));
        self.words()
    }

    fn words(&self) -> &[u16] {
        let slot = &self.pool.slots[self.index];
        // SAFETY: the slot is READING; no write lease can exist for it.
        unsafe { core::slice::from_raw_parts(slot.ptr, slot.len) }
    }

    /// A color-aware source retaining the publication's explicit byte order.
    /// The returned view cannot outlive this lease, including during DMA.
    pub fn source(&self, size: (u16, u16)) -> PixelSource<'_> {
        let words = self.words();
        if self.pool.slots[self.index]
            .big_endian
            .load(Ordering::Relaxed)
        {
            // SAFETY: u16 storage has no padding, is initialized by the write
            // lease, and remains immutable while this read lease exists.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    words.as_ptr().cast::<u8>(),
                    core::mem::size_of_val(words),
                )
            };
            PixelSource::bytes(bytes, size, size.0 as usize * 2, PixelFormat::Rgb565Be)
        } else {
            PixelSource::rgb565(words, size)
        }
    }

    /// Raw address for LVGL's canvas descriptor (host preview path).
    pub fn as_ptr(&self) -> *const u16 {
        assert!(!self.pool.slots[self.index]
            .big_endian
            .load(Ordering::Relaxed));
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
    fn wire_publication_borrows_exact_bytes_and_recycled_native_frame_resets_encoding() {
        use board_s31::geometry::DirtyRect;
        let pool = pool();
        let colors: alloc::vec::Vec<u16> = (0..16).map(|i| i * 4093).collect();
        let mut write = pool.acquire().unwrap();
        for (out, &color) in write.pixels_mut().iter_mut().zip(&colors) {
            *out = color.to_be();
        }
        write.publish_rgb565be();
        let wire = pool.take_ready().unwrap();
        let source = wire.source((4, 4));
        let area = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 3,
            y1: 3,
        };
        let direct = source.contiguous(area, PixelFormat::Rgb565Be).unwrap();
        let expected: alloc::vec::Vec<u8> = colors.iter().flat_map(|p| p.to_be_bytes()).collect();
        assert_eq!(direct, expected);
        assert_eq!(direct.as_ptr(), wire.words().as_ptr().cast::<u8>());
        for (i, &color) in colors.iter().enumerate() {
            assert_eq!(source.rgb565_at(i % 4, i / 4), color);
        }
        // Hold the other slot so the just-released wire slot is reused.
        let other = pool.acquire().unwrap();
        drop(wire);
        let mut write = pool.acquire().unwrap();
        write.pixels_mut().copy_from_slice(&colors);
        write.publish();
        let native = pool.take_ready().unwrap();
        assert_eq!(native.pixels(), colors);
        assert!(native
            .source((4, 4))
            .contiguous(area, PixelFormat::Rgb565Be)
            .is_none());
        drop(other);
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
