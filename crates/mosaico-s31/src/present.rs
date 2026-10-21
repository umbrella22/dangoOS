//! Two-frame presentation handshake between the LVGL UI task and the panel
//! display task.
//!
//! # Frame ownership
//!
//! Two full-frame RGB565 buffers live in PSRAM. At every instant [`Core`]
//! assigns each buffer exactly one role:
//!
//! - [`FrameState::Back`]: the UI task may paint it through [`Presenter::flush_area`];
//!   the display task never reads it.
//! - [`FrameState::InFlight`]: the display task may read it through
//!   [`Presenter::front_pixels`]; the UI task never writes it.
//! - [`FrameState::Free`]: nobody accesses it; it can become the next back frame.
//!
//! `submit` moves the back buffer to `InFlight` and, when the other buffer is
//! `Free`, immediately makes it the new back buffer. When the display task has
//! not released the previous frame yet, no buffer is handed out;
//! [`Presenter::wait_back_free`] blocks until `release` publishes one. The UI
//! can therefore run at most two frames ahead of the panel and never overwrites
//! a frame that is still being transferred, while a single submit-and-wait
//! cycle does not serialize rendering against the QSPI transfer.
//!
//! The first frame starts as `Back` and the second as `Free`, so
//! `wait_back_free` returns immediately when the display task is idle.
//!
//! # Safety
//!
//! [`Shared`] is reachable from both tasks and therefore needs an
//! `unsafe impl Sync`. The ownership invariant above justifies it:
//!
//! - [`Presenter::flush_area`] copies into the frame reported by `Core::back`;
//!   the UI-side `Presenter` is not `Clone`/`Copy`, so there is one writer.
//!   The copy uses a raw pointer derived from the leaked `PixelMemory`, and no
//!   `&u16` slice to that allocation is alive while the frame is `Back`.
//! - [`Presenter::front_pixels`] returns a shared slice for a frame returned by
//!   `next_job`. `release` takes `&mut self`, so the borrow checker prevents
//!   releasing the same handle while that slice is still alive. The display
//!   side must call `release` only after it has stopped reading.
//! - A frame is never `Back` and `InFlight` at the same time. `Core` is the
//!   only transition authority and every transition runs inside the
//!   `CriticalSectionRawMutex`, which also makes the transitions visible to
//!   both tasks.
//!
//! No LVGL pointer is retained: `flush_area` copies immediately.
//!
//! # Touch-tagged frames
//!
//! The UI task stores the timestamp of a press edge with
//! [`Presenter::tag_next_frame`]. The tag is consumed by the next
//! [`Presenter::submit`], so exactly one [`FrameJob`] carries the original
//! touch timestamp and `display_task` can turn it into one latency sample.
//! Frames that were already submitted (including animation frames rendered
//! before the press) can never consume the sample. A tag that is not followed
//! by a submission stays pending until the next frame is submitted; if that
//! frame has no damage, no sample is recorded because no QSPI byte was sent.

#![allow(dead_code)]

use core::{cell::RefCell, slice};

use alloc::boxed::Box;
use embassy_sync::{
    blocking_mutex::{raw::CriticalSectionRawMutex, Mutex},
    channel::Channel,
    signal::Signal,
};

use crate::{
    framebuffer::{HEIGHT, WIDTH},
    memory::PixelMemory,
};

pub use crate::framebuffer::DirtyRect;

pub const FRAME_WIDTH: usize = WIDTH;
pub const FRAME_HEIGHT: usize = HEIGHT;
pub const FRAME_PIXELS: usize = FRAME_WIDTH * FRAME_HEIGHT;
pub const FRAME_BYTES: usize = FRAME_PIXELS * 2;

/// Number of presentation frames; the handshake is defined for exactly two.
const FRAME_COUNT: usize = 2;

/// One of the two presentation buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameId(pub u8);

impl FrameId {
    pub const A: Self = Self(0);
    pub const B: Self = Self(1);

    const fn index(self) -> usize {
        self.0 as usize
    }

    const fn is_valid(self) -> bool {
        self.0 < FRAME_COUNT as u8
    }

    const fn peer(self) -> Self {
        match self.0 {
            0 => Self(1),
            1 => Self(0),
            _ => panic!("invalid FrameId"),
        }
    }
}

/// Ownership role of one presentation frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameState {
    Free,
    Back,
    InFlight,
}

/// A published frame and the damage that must be transferred for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameJob {
    pub id: FrameId,
    /// Union of everything flushed into this frame, aligned to 4 pixels in x.
    pub damage: DirtyRect,
    /// Touch timestamp carried by this frame, if the UI task tagged it.
    ///
    /// This is the `at_us` of the press edge that the frame responds to, not
    /// the time of submission. `display_task` records one latency sample when
    /// it starts the first transfer of a tagged job.
    pub touch_at_us: Option<u64>,
}

/// Pure frame-ownership and damage state machine.
///
/// This type contains no ESP types and no interior mutability, so it can be
/// host-tested and later moved into the library surface by R1.3. All methods
/// are synchronous; the async handshake wraps this type with `embassy-sync`
/// channels.
#[derive(Clone, Debug)]
pub struct Core {
    states: [FrameState; FRAME_COUNT],
    damage: [DirtyRect; FRAME_COUNT],
}

impl Core {
    /// Frame A is the initial back buffer, frame B is free.
    pub const fn new() -> Self {
        Self {
            states: [FrameState::Back, FrameState::Free],
            damage: [DirtyRect::EMPTY; FRAME_COUNT],
        }
    }

    pub fn state(&self, id: FrameId) -> Option<FrameState> {
        if id.is_valid() {
            Some(self.states[id.index()])
        } else {
            None
        }
    }

    /// The buffer the UI may paint, if any. `None` means both frames are in
    /// flight and `wait_back_free` must block.
    pub fn back(&self) -> Option<FrameId> {
        let mut index = 0;
        while index < FRAME_COUNT {
            if self.states[index] == FrameState::Back {
                return Some(FrameId(index as u8));
            }
            index += 1;
        }
        None
    }

    /// Record a flushed area against the current back frame.
    ///
    /// Returns `false` when there is no back frame or the area is invalid.
    pub fn mark_flushed(&mut self, area: DirtyRect) -> bool {
        let Some(id) = self.back() else {
            return false;
        };
        if !area.is_valid() {
            return false;
        }
        let slot = id.index();
        self.damage[slot] = self.damage[slot].union(area);
        true
    }

    /// Publish the back frame as a [`FrameJob`] and select the next back frame.
    ///
    /// The published damage is the union of all flushed areas plus `damage`,
    /// rounded outward to 4-pixel x alignment. Returns `None` when there is no
    /// back frame. When the peer frame is still in flight, no back frame is
    /// handed out until `release` is called: the UI has reached its
    /// back-pressure limit.
    pub fn submit(&mut self, damage: DirtyRect) -> Option<FrameJob> {
        let id = self.back()?;
        let extra = if damage.is_valid() {
            damage
        } else {
            DirtyRect::EMPTY
        };
        let slot = id.index();
        let merged = self.damage[slot].union(extra).align_x4();
        self.damage[slot] = DirtyRect::EMPTY;
        self.states[slot] = FrameState::InFlight;

        let peer = id.peer().index();
        if self.states[peer] == FrameState::Free {
            self.states[peer] = FrameState::Back;
        }

        Some(FrameJob {
            id,
            damage: merged,
            touch_at_us: None,
        })
    }

    /// Return an in-flight frame to the pool.
    ///
    /// If the UI is currently starving for a back frame (both frames were in
    /// flight), the released frame becomes the new back frame. Returns `false`
    /// when `id` was not in flight, which makes a duplicate release harmless.
    pub fn release(&mut self, id: FrameId) -> bool {
        if !id.is_valid() || self.states[id.index()] != FrameState::InFlight {
            return false;
        }
        self.states[id.index()] = FrameState::Free;
        if self.back().is_none() {
            self.states[id.index()] = FrameState::Back;
        }
        true
    }
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

struct Shared {
    frames: [PixelMemory; FRAME_COUNT],
    core: Mutex<CriticalSectionRawMutex, RefCell<Core>>,
    jobs: Channel<CriticalSectionRawMutex, FrameJob, FRAME_COUNT>,
    released: Signal<CriticalSectionRawMutex, FrameId>,
}

// SAFETY: see the module-level "Safety" section. `Core` transitions are
// serialized by `core`, and the raw frame bytes are only reached by the role
// that `Core` assigns: the single UI `Presenter` for `Back`, and display
// handles for `InFlight`. Frames are never dropped because `Shared` is leaked.
unsafe impl Sync for Shared {}

impl Shared {
    async fn next_job(&self) -> FrameJob {
        self.jobs.receive().await
    }

    /// Take an already-published job without blocking.
    ///
    /// The display task uses this while waiting for a panel frame boundary: if
    /// a newer job is already queued (typically the touch response that must
    /// not be delayed), it is handled first instead of waiting.
    fn try_next_job(&self) -> Option<FrameJob> {
        self.jobs.try_receive().ok()
    }

    fn front_pixels(&self, id: FrameId) -> &[u16] {
        assert!(id.is_valid(), "invalid FrameId");
        let state = self.core.lock(|cell| cell.borrow().state(id));
        assert_eq!(
            state,
            Some(FrameState::InFlight),
            "front_pixels requires a frame returned by next_job",
        );
        // SAFETY: the check above and the handshake ensure the frame is
        // `InFlight`, so only the display side reads it. The allocation is
        // exactly FRAME_PIXELS initialized u16 values and lives as long as the
        // leaked `Shared`.
        unsafe {
            slice::from_raw_parts(self.frames[id.index()].as_ptr().cast::<u16>(), FRAME_PIXELS)
        }
    }

    fn release(&self, id: FrameId) {
        let released = self.core.lock(|cell| cell.borrow_mut().release(id));
        if released {
            self.released.signal(id);
        }
    }
}

/// UI-side presentation handle.
///
/// Construct it once, move it into the UI task, and pass
/// [`Presenter::display_side`] to the display task. This type is deliberately
/// not `Clone`/`Copy`: exactly one task may call the UI-side methods, which is
/// what makes the raw frame writes sound.
pub struct Presenter {
    shared: &'static Shared,
    /// Touch timestamp that the next submitted frame must carry. Set by the UI
    /// task after it applied a press edge; consumed by `submit`.
    pending_touch: Option<u64>,
}

impl Presenter {
    /// Allocate both presentation frames in PSRAM and leak the shared state.
    ///
    /// The allocator (PSRAM included) must already be installed. Calling this
    /// twice creates two independent presenters; the firmware calls it once.
    pub fn new() -> Self {
        let shared = Box::leak(Box::new(Shared {
            frames: [
                PixelMemory::new(FRAME_BYTES, true),
                PixelMemory::new(FRAME_BYTES, true),
            ],
            core: Mutex::new(RefCell::new(Core::new())),
            jobs: Channel::new(),
            released: Signal::new(),
        }));
        Self {
            shared,
            pending_touch: None,
        }
    }

    /// Tag the next submitted frame with a touch timestamp.
    ///
    /// The UI task calls this when it drains a press edge, *after* the press
    /// has been applied to the application state and before the render that
    /// produces the response frame. [`Presenter::submit`] transfers the tag
    /// into the [`FrameJob`] and clears it. If several presses arrive before a
    /// frame is submitted, the earliest timestamp wins: the response frame
    /// then measures the longest wait, which is the conservative sample.
    pub fn tag_next_frame(&mut self, at_us: u64) {
        if self.pending_touch.is_none() {
            self.pending_touch = Some(at_us);
        }
    }

    /// A display-only handle for the display task.
    ///
    /// Display handlers only wait for jobs, read the published frame, and
    /// release it. They never write frame pixels.
    pub fn display_side(&self) -> DisplayPresenter {
        DisplayPresenter {
            shared: self.shared,
        }
    }

    /// The frame the UI may paint right now.
    ///
    /// Call [`Presenter::wait_back_free`] first; this panics if both frames are
    /// in flight.
    pub fn back_frame(&self) -> FrameId {
        self.with_core(|core| core.back())
            .expect("no back frame; call wait_back_free first")
    }

    /// Wait until a back frame is available.
    ///
    /// Returns immediately when the display task is idle (frame A starts as the
    /// back buffer and frame B is free).
    pub async fn wait_back_free(&mut self) {
        loop {
            if self.with_core(|core| core.back()).is_some() {
                return;
            }
            self.shared.released.wait().await;
        }
    }

    /// Copy one flushed LVGL area into the back frame.
    ///
    /// `pixels` is tightly packed row-major RGB565 for exactly `area`
    /// (stride = `area.width()` pixels). Nothing is retained; LVGL may reuse
    /// its draw buffer when this returns. Empty areas are no-ops.
    pub fn flush_area(&mut self, area: DirtyRect, pixels: &[u16]) {
        self.flush_area_strided(area, pixels, area.width());
    }

    /// Strided variant of [`Presenter::flush_area`] for an LVGL draw buffer
    /// whose row stride is wider than the flushed area.
    ///
    /// `stride` is in pixels and must be at least `area.width()`. This is an
    /// additive extension; the frozen interface only requires `flush_area`.
    pub fn flush_area_strided(&mut self, area: DirtyRect, pixels: &[u16], stride: usize) {
        if area.is_empty() {
            return;
        }
        assert!(area.is_valid(), "flush area outside the panel: {area:?}");
        let width = area.width();
        let height = area.height();
        assert!(stride >= width, "flush stride is narrower than the area");
        let required = (height - 1) * stride + width;
        assert!(
            pixels.len() >= required,
            "flush source has {} pixels, need {required}",
            pixels.len(),
        );

        let ptr = self.with_core(|core| {
            let id = core
                .back()
                .expect("flush_area without a back frame; call wait_back_free first");
            core.mark_flushed(area);
            self.shared.frames[id.index()].as_ptr().cast::<u16>()
        });

        // SAFETY: `core` assigned this frame the `Back` role. The single
        // UI-side `Presenter` is the only writer; no slice into this
        // allocation exists because the display side may only read `InFlight`
        // frames. `ptr` addresses exactly FRAME_PIXELS initialized u16 values.
        let frame = unsafe { slice::from_raw_parts_mut(ptr, FRAME_PIXELS) };
        for row in 0..height {
            let source = &pixels[row * stride..row * stride + width];
            let target = (area.y0 as usize + row) * FRAME_WIDTH + area.x0 as usize;
            frame[target..target + width].copy_from_slice(source);
        }
    }

    /// Publish the accumulated damage and hand the frame to the display side.
    ///
    /// Returns the published [`FrameId`]. A touch tag set by
    /// [`Presenter::tag_next_frame`] is attached to this job and cleared, even
    /// when the frame carries no damage. The next back frame is the peer frame
    /// if it is free; otherwise [`Presenter::wait_back_free`] blocks until the
    /// display task releases one.
    pub fn submit(&mut self, damage: DirtyRect) -> FrameId {
        let touch_at_us = self.pending_touch.take();
        let mut job = self
            .with_core(|core| core.submit(damage))
            .expect("submit without a back frame; call wait_back_free first");
        job.touch_at_us = touch_at_us;
        // The channel capacity equals the frame count, and at most one job per
        // frame can exist, so this cannot fail unless the protocol is broken.
        self.shared
            .jobs
            .try_send(job)
            .expect("presentation job queue overflow");
        job.id
    }

    /// Display side: wait for the next published frame.
    pub async fn next_job(&mut self) -> FrameJob {
        self.shared.next_job().await
    }

    /// Take a queued job without blocking, if one is already published.
    pub fn try_next_job(&mut self) -> Option<FrameJob> {
        self.shared.try_next_job()
    }

    /// Display side: read a published frame.
    ///
    /// The returned slice borrows this handle; call [`Presenter::release`] only
    /// after the last read.
    pub fn front_pixels(&self, id: FrameId) -> &[u16] {
        self.shared.front_pixels(id)
    }

    /// Display side: return a frame to the pool.
    pub fn release(&mut self, id: FrameId) {
        self.shared.release(id);
    }

    fn with_core<R>(&self, f: impl FnOnce(&mut Core) -> R) -> R {
        self.shared.core.lock(|cell| f(&mut cell.borrow_mut()))
    }
}

impl Default for Presenter {
    fn default() -> Self {
        Self::new()
    }
}

/// Display-side presentation handle produced by [`Presenter::display_side`].
///
/// It can only wait for jobs, read published frames and release them; it has no
/// way to write frame pixels.
pub struct DisplayPresenter {
    shared: &'static Shared,
}

impl DisplayPresenter {
    /// Wait for the next published frame.
    pub async fn next_job(&mut self) -> FrameJob {
        self.shared.next_job().await
    }

    /// Take a queued job without blocking, if one is already published.
    pub fn try_next_job(&mut self) -> Option<FrameJob> {
        self.shared.try_next_job()
    }

    /// Read a published frame. The slice borrows this handle, so
    /// [`DisplayPresenter::release`] cannot be called while it is alive.
    pub fn front_pixels(&self, id: FrameId) -> &[u16] {
        self.shared.front_pixels(id)
    }

    /// Return the frame to the pool once the transfer has finished.
    pub fn release(&mut self, id: FrameId) {
        self.shared.release(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: u16, y0: u16, x1: u16, y1: u16) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn initial_back_frame_is_available_and_peer_is_free() {
        let core = Core::new();
        assert_eq!(core.back(), Some(FrameId::A));
        assert_eq!(core.state(FrameId::A), Some(FrameState::Back));
        assert_eq!(core.state(FrameId::B), Some(FrameState::Free));
    }

    #[test]
    fn two_frames_alternate_under_release() {
        let mut core = Core::new();
        assert_eq!(core.submit(DirtyRect::EMPTY).unwrap().id, FrameId::A);
        assert_eq!(core.back(), Some(FrameId::B));
        assert!(core.release(FrameId::A));
        assert_eq!(core.submit(DirtyRect::EMPTY).unwrap().id, FrameId::B);
        // A is free again, so the peer of B becomes the back frame.
        assert_eq!(core.back(), Some(FrameId::A));
        assert!(core.release(FrameId::B));
        assert_eq!(core.submit(DirtyRect::EMPTY).unwrap().id, FrameId::A);
        assert_eq!(core.back(), Some(FrameId::B));
    }

    #[test]
    fn submit_while_other_frame_in_flight_blocks_until_release() {
        let mut core = Core::new();
        assert_eq!(core.submit(DirtyRect::EMPTY).unwrap().id, FrameId::A);
        assert_eq!(core.submit(DirtyRect::EMPTY).unwrap().id, FrameId::B);
        // Both frames are in flight: the UI has no writable frame.
        assert_eq!(core.back(), None);
        assert_eq!(core.state(FrameId::A), Some(FrameState::InFlight));
        assert_eq!(core.state(FrameId::B), Some(FrameState::InFlight));

        // Releasing the older frame makes it the new back frame.
        assert!(core.release(FrameId::A));
        assert_eq!(core.back(), Some(FrameId::A));
        assert_eq!(core.state(FrameId::A), Some(FrameState::Back));
        assert_eq!(core.state(FrameId::B), Some(FrameState::InFlight));
    }

    #[test]
    fn release_frees_a_frame_when_a_back_frame_exists() {
        let mut core = Core::new();
        core.submit(DirtyRect::EMPTY).unwrap();
        assert!(core.release(FrameId::A));
        assert_eq!(core.state(FrameId::A), Some(FrameState::Free));
        assert_eq!(core.back(), Some(FrameId::B));
    }

    #[test]
    fn no_frame_is_handed_out_twice() {
        let mut core = Core::new();
        let first = core.submit(DirtyRect::EMPTY).unwrap();
        let second = core.submit(DirtyRect::EMPTY).unwrap();
        assert_ne!(first.id, second.id);

        // While A is in flight it can never be the back frame.
        assert_eq!(core.back(), None);
        assert!(core.release(FrameId::A));
        assert_eq!(core.back(), Some(FrameId::A));
        assert_ne!(core.back(), Some(FrameId::B));

        // A duplicate release is harmless and does not publish a second copy.
        assert!(!core.release(FrameId::A));
        assert_eq!(core.state(FrameId::A), Some(FrameState::Back));

        // Invalid ids are ignored.
        assert!(!core.release(FrameId(2)));
        assert_eq!(core.state(FrameId(2)), None);
    }

    #[test]
    fn damage_unions_and_aligns_at_submit() {
        let mut core = Core::new();
        assert!(core.mark_flushed(rect(10, 10, 20, 20)));
        assert!(core.mark_flushed(rect(30, 5, 40, 8)));
        assert!(core.mark_flushed(rect(110, 100, 120, 110)));
        let job = core.submit(rect(0, 200, 3, 200)).unwrap();
        // x0 rounds down to 0, x1 rounds up to 123; y spans 5..=200.
        assert_eq!(job.damage, rect(0, 5, 123, 200));
        assert_eq!(job.damage.y0, 5);
        assert_eq!(job.damage.y1, 200);
    }

    #[test]
    fn empty_and_invalid_damage_are_ignored() {
        let mut core = Core::new();
        assert!(!core.mark_flushed(DirtyRect::EMPTY));
        assert!(!core.mark_flushed(rect(0, 0, 480, 1)));
        let job = core.submit(DirtyRect::EMPTY).unwrap();
        assert_eq!(job.damage, DirtyRect::EMPTY);
    }

    #[test]
    fn submit_without_back_frame_returns_none() {
        let mut core = Core::new();
        core.submit(DirtyRect::EMPTY).unwrap();
        core.submit(DirtyRect::EMPTY).unwrap();
        assert_eq!(core.back(), None);
        assert_eq!(core.submit(DirtyRect::EMPTY), None);
        assert!(!core.mark_flushed(rect(0, 0, 3, 3)));
    }

    #[test]
    fn flush_area_copies_only_the_back_frame() {
        let mut presenter = Presenter::new();
        assert_eq!(presenter.back_frame(), FrameId::A);

        let area = rect(5, 7, 7, 8);
        let pixels = [0x1111u16, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666];
        presenter.flush_area(area, &pixels);

        let id = presenter.submit(DirtyRect::EMPTY);
        assert_eq!(id, FrameId::A);
        let front = presenter.front_pixels(id);
        assert_eq!(front[7 * FRAME_WIDTH + 5], 0x1111);
        assert_eq!(front[7 * FRAME_WIDTH + 7], 0x3333);
        assert_eq!(front[8 * FRAME_WIDTH + 5], 0x4444);
        assert_eq!(front[8 * FRAME_WIDTH + 7], 0x6666);
        assert_eq!(front[7 * FRAME_WIDTH + 4], 0);
        assert_eq!(front[8 * FRAME_WIDTH + 8], 0);
        assert_eq!(front[9 * FRAME_WIDTH + 5], 0);

        // Publish the peer frame as well; it must still be untouched.
        let peer_id = presenter.submit(DirtyRect::EMPTY);
        assert_eq!(peer_id, FrameId::B);
        let peer = presenter.front_pixels(peer_id);
        assert!(peer.iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn strided_flush_skips_padding_pixels() {
        let mut presenter = Presenter::new();
        let area = rect(0, 0, 2, 1);
        // Two rows with a 5-pixel stride and one padding pixel per row.
        let pixels = [1u16, 2, 3, 0, 0, 4, 5, 6];
        presenter.flush_area_strided(area, &pixels, 5);

        let id = presenter.submit(DirtyRect::EMPTY);
        let front = presenter.front_pixels(id);
        assert_eq!(&front[0..3], &[1, 2, 3]);
        assert_eq!(&front[FRAME_WIDTH..FRAME_WIDTH + 3], &[4, 5, 6]);
        assert_eq!(front[3], 0);
    }

    #[test]
    fn touch_tag_attaches_to_the_next_submitted_job_only() {
        let mut presenter = Presenter::new();
        let mut display = presenter.display_side();

        presenter.tag_next_frame(1_234);
        presenter.submit(DirtyRect::EMPTY);
        let tagged = display.shared.jobs.try_receive().unwrap();
        assert_eq!(tagged.touch_at_us, Some(1_234));

        // The tag is consumed by the first submit.
        presenter.submit(DirtyRect::EMPTY);
        let plain = display.shared.jobs.try_receive().unwrap();
        assert_eq!(plain.touch_at_us, None);
    }

    #[test]
    fn earliest_pending_touch_tag_wins() {
        let mut presenter = Presenter::new();
        let mut display = presenter.display_side();

        presenter.tag_next_frame(100);
        presenter.tag_next_frame(200);
        presenter.submit(DirtyRect::EMPTY);
        let job = display.shared.jobs.try_receive().unwrap();
        assert_eq!(job.touch_at_us, Some(100));
    }

    #[test]
    fn display_side_observes_submitted_jobs() {
        let mut presenter = Presenter::new();
        let mut display = presenter.display_side();
        presenter.flush_area(rect(0, 0, 3, 0), &[0x1234; 4]);
        presenter.submit(DirtyRect::EMPTY);

        let job = display.shared.jobs.try_receive().unwrap();
        assert_eq!(job.id, FrameId::A);
        assert_eq!(job.damage, rect(0, 0, 3, 0));
        assert_eq!(display.front_pixels(job.id)[0], 0x1234);
        display.release(job.id);
        // The peer frame was already the back frame, so releasing the
        // transferred frame leaves it free for the next submit.
        assert_eq!(presenter.back_frame(), FrameId::B);
    }
}
