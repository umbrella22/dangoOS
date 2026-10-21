//! Native LVGL RGB565 display port feeding the two-frame presenter.
//!
//! LVGL renders in PARTIAL mode into one 480x48 internal-RAM draw buffer. The
//! flush callback immediately copies every flushed area into the presenter's
//! current back frame and accumulates its rectangle; when LVGL reports the last
//! flush of a refresh, the accumulated damage is submitted as one
//! [`FrameJob`] for `display_task`.
//!
//! No LVGL pointer is retained past [`sys::lv_display_flush_ready`].
//!
//! # Two-frame partial-flush correctness
//!
//! The presenter keeps exactly two retained frames. With PARTIAL rendering, an
//! area changed while the peer frame was current is absent from the recycled
//! frame, so alternating frames would show stale pixels (see
//! `docs/plans/r1-runtime-design.md`, "Two-frame partial-flush caveat").
//!
//! The ball screen avoids that by construction: `ui_task` invalidates the full
//! screen only for the first two frames (to paint the static black field into
//! both buffers), and afterwards `BallWidget` repaints and invalidates its
//! entire canvas. Each refresh therefore carries damage that covers every
//! pixel that changed, and the static background is already identical in both
//! frames. R1's original every-refresh full invalidation would copy the whole
//! 480x480 panel for a 240x240 canvas and is no longer used.

use alloc::boxed::Box;
use core::{cell::RefCell, ptr::NonNull, slice};

use lvgl::sys;

use crate::{framebuffer::DirtyRect, memory::PixelMemory, present::Presenter};

/// Flush rows per partial draw buffer. Ten flushes cover the 480-row panel.
pub const DRAW_LINES: usize = 48;

/// LVGL refresh timer period. `ui_task` also runs its loop every 5 ms.
pub const REFRESH_PERIOD_MS: u32 = 10;

/// Counters collected by the flush callback (reset by [`DisplayPort::take_flush_stats`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct FlushStats {
    /// Number of flush callbacks (partial areas copied into the back frame).
    pub flushes: u64,
    /// Number of completed refreshes submitted to the display task.
    pub frames: u64,
    /// Pixels copied by all flushes in this window.
    pub pixels: u64,
}

struct State {
    presenter: Presenter,
    /// Union of areas flushed into the current back frame.
    damage: DirtyRect,
    stats: FlushStats,
    /// Bounds of the LVGL draw allocation, used to validate `px_map`.
    draw_start: usize,
    draw_len: usize,
}

/// The firmware's sole LVGL display, owned by the UI task.
pub struct DisplayPort {
    display: NonNull<sys::lv_display_t>,
    state: Box<RefCell<State>>,
    // C retains only this allocation's address, not a Rust reference.
    _draw: PixelMemory,
}

impl DisplayPort {
    /// Register the native LVGL display and take ownership of the UI-side
    /// [`Presenter`].
    ///
    /// # Safety
    ///
    /// LVGL must already be initialized and every LVGL call must be serialized
    /// on one thread (the UI task). No native display/widget handle may outlive
    /// this value, and the process must not delete the display separately.
    pub unsafe fn new(presenter: Presenter) -> Self {
        let format = sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565;
        // A PARTIAL draw buffer may have a padded stride; derive it exactly the
        // way LVGL does instead of assuming 480 * 2 bytes.
        let stride =
            unsafe { sys::lv_draw_buf_width_to_stride(crate::framebuffer::WIDTH as u32, format) }
                as usize;
        assert!(
            stride >= crate::framebuffer::WIDTH * 2,
            "LVGL RGB565 stride is narrower than the panel"
        );
        let draw = PixelMemory::new(stride * DRAW_LINES, false);

        let state = Box::new(RefCell::new(State {
            presenter,
            damage: DirtyRect::EMPTY,
            stats: FlushStats::default(),
            draw_start: draw.as_ptr() as usize,
            draw_len: draw.len(),
        }));

        let display = NonNull::new(unsafe {
            sys::lv_display_create(
                crate::framebuffer::WIDTH as i32,
                crate::framebuffer::HEIGHT as i32,
            )
        })
        .expect("LVGL display allocation failed");
        unsafe {
            sys::lv_display_set_color_format(display.as_ptr(), format);
            sys::lv_display_set_buffers(
                display.as_ptr(),
                draw.as_ptr().cast(),
                core::ptr::null_mut(),
                draw.len() as u32,
                sys::lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL,
            );
            sys::lv_display_set_user_data(
                display.as_ptr(),
                (&*state as *const RefCell<State>).cast_mut().cast(),
            );
            sys::lv_display_set_flush_cb(display.as_ptr(), Some(flush));
            let timer = sys::lv_display_get_refr_timer(display.as_ptr());
            assert!(!timer.is_null(), "LVGL refresh timer allocation failed");
            sys::lv_timer_set_period(timer, REFRESH_PERIOD_MS);
        }

        Self {
            display,
            state,
            _draw: draw,
        }
    }

    pub fn as_raw(&self) -> *mut sys::lv_display_t {
        self.display.as_ptr()
    }

    /// The display's active screen. Used to invalidate the full panel.
    pub fn screen(&self) -> NonNull<sys::lv_obj_t> {
        NonNull::new(unsafe { sys::lv_display_get_screen_active(self.display.as_ptr()) })
            .expect("LVGL display has no active screen")
    }

    /// Invalidate the whole screen so the next refresh repaints every pixel in
    /// both retained frames (see the module-level caveat).
    pub fn invalidate_full(&self) {
        unsafe { sys::lv_obj_invalidate(self.screen().as_ptr()) };
    }

    /// Tag the next submitted frame with a press-edge timestamp.
    ///
    /// `ui_task` calls this after the press has been applied to the
    /// application state and before `lv_timer_handler` renders the response.
    /// The tag travels through [`crate::present::FrameJob`], so only the
    /// response frame can produce a latency sample.
    pub fn tag_next_frame(&self, at_us: u64) {
        self.state.borrow_mut().presenter.tag_next_frame(at_us);
    }

    /// Wait until the presenter has a writable back frame.
    ///
    /// The borrow of the port state is held across the await. That is sound
    /// here because no other task ever calls into the port (LVGL is owned by
    /// the UI task) and the flush callback only runs from `lv_timer_handler`,
    /// which is never called while this future is suspended.
    pub async fn wait_back_free(&self) {
        let mut state = self.state.borrow_mut();
        state.presenter.wait_back_free().await;
    }

    pub fn take_flush_stats(&self) -> FlushStats {
        core::mem::take(&mut self.state.borrow_mut().stats)
    }

    /// Number of refreshes submitted to the display task since boot.
    ///
    /// Unlike [`DisplayPort::take_flush_stats`] this does not reset a window;
    /// `ui_task` uses the delta around `timer_handler` to time only the loops
    /// that actually rendered and submitted a frame.
    pub fn frame_count(&self) -> u64 {
        self.state.borrow().stats.frames
    }
}

impl Drop for DisplayPort {
    fn drop(&mut self) {
        // Remove C's callbacks before releasing the draw allocation and state.
        // The UI task never drops this value in R1 (it loops forever), but the
        // teardown order is still correct.
        unsafe { sys::lv_display_delete(self.display.as_ptr()) };
    }
}

unsafe extern "C" fn flush(
    display: *mut sys::lv_display_t,
    area: *const sys::lv_area_t,
    pixels: *mut u8,
) {
    let user_data = unsafe { sys::lv_display_get_user_data(display) };
    if user_data.is_null() {
        unsafe { sys::lv_display_flush_ready(display) };
        return;
    }
    // SAFETY: `user_data` points at the `RefCell<State>` owned by the live
    // DisplayPort; LVGL invokes this callback synchronously from the UI task.
    let cell = unsafe { &*user_data.cast::<RefCell<State>>() };
    let raw_area = unsafe { &*area };
    assert!(
        raw_area.x1 >= 0
            && raw_area.y1 >= 0
            && raw_area.x2 >= raw_area.x1
            && raw_area.y2 >= raw_area.y1,
        "invalid LVGL flush area: ({}, {})-({}, {})",
        raw_area.x1,
        raw_area.y1,
        raw_area.x2,
        raw_area.y2,
    );
    let area = DirtyRect {
        x0: raw_area.x1 as u16,
        y0: raw_area.y1 as u16,
        x1: raw_area.x2 as u16,
        y1: raw_area.y2 as u16,
    };
    assert!(area.is_valid(), "LVGL flush outside the panel: {area:?}");

    let active = unsafe { sys::lv_display_get_buf_active(display) };
    assert!(!active.is_null(), "LVGL has no active draw buffer");
    // The stride belongs to the PARTIAL draw buffer, not the full display.
    let stride = unsafe { (*active).header.stride() } as usize;
    assert!(
        stride % 2 == 0 && stride >= area.width() * 2,
        "unexpected LVGL RGB565 stride {stride}"
    );
    let bytes = (area.height() - 1)
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add(area.width() * 2))
        .expect("LVGL flush size overflow");
    assert_eq!(
        (pixels as usize) % core::mem::align_of::<u16>(),
        0,
        "LVGL RGB565 draw buffer is not u16-aligned"
    );

    let mut state = cell.borrow_mut();
    let offset = (pixels as usize)
        .checked_sub(state.draw_start)
        .expect("LVGL flush pointer precedes the draw buffer");
    assert!(
        offset <= state.draw_len && bytes <= state.draw_len - offset,
        "LVGL flush exceeds the draw allocation"
    );
    // SAFETY: LVGL finished writing this region and cannot reuse it until
    // `lv_display_flush_ready` below; the validated bounds prove the slice is
    // inside the live 128-byte-aligned draw allocation. The pointer is
    // u16-aligned and every bit pattern is a valid RGB565 pixel.
    let source = unsafe { slice::from_raw_parts(pixels.cast::<u16>(), bytes / 2) };
    state.presenter.flush_area_strided(area, source, stride / 2);
    state.damage = state.damage.union(area);
    state.stats.flushes += 1;
    state.stats.pixels += area.area() as u64;

    if unsafe { sys::lv_display_flush_is_last(display) } {
        let damage = core::mem::replace(&mut state.damage, DirtyRect::EMPTY);
        state.presenter.submit(damage);
        state.stats.frames += 1;
    }
    drop(state);
    // Completion means the copy into the retained frame finished, not that the
    // QSPI transfer finished; the display task has its own release handshake.
    unsafe { sys::lv_display_flush_ready(display) };
}
