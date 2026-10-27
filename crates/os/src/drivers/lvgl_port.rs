//! LVGL display and pointer registration for the hub pages (RFC 0006).
//!
//! The fox home page never goes through LVGL rendering: its canvas is
//! uploaded by `plane_task` straight from the canvas pool. The launcher and
//! the native apps are ordinary LVGL widget trees, rendered here:
//!
//! - LVGL renders in PARTIAL mode into a small internal-RAM draw buffer
//!   ([`DRAW_ROWS`] rows), which keeps the rasterizer on fast memory;
//! - the flush callback copies each band into a 480x480 PSRAM shadow frame
//!   and grows a damage rectangle;
//! - the UI task then hands the shadow to the plane as a [`ScreenLease`]
//!   with that rectangle. While the lease lives the shadow is "busy" and the
//!   UI task does not run `lv_timer_handler`, so LVGL never writes pixels the
//!   panel DMA is still reading.
//!
//! Page slides (`hub::transition`) use a second PSRAM frame, the snapshot:
//! [`DisplayPort::snapshot`] copies the old page into it, the new page is
//! rendered into the shadow with [`DisplayPort::render_now`], and the plane
//! uploads bands of both through [`DisplayPort::take_slide`]. The snapshot
//! has its own busy flag; it is only rewritten while no lease of it exists.
//!
//! Touch reaches LVGL through a pointer input device whose read callback
//! reports the latest sample set by [`set_pointer`]. A press shorter than
//! LVGL's 33 ms read period is latched so taps are never lost.

use core::cell::Cell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

use critical_section::Mutex;
use rs_lvgl::sys;

use crate::drivers::geometry::{DirtyRect, HEIGHT, WIDTH};
use crate::drivers::memory::PixelMemory;

/// Rows of the internal draw buffer (480 x 48 x 2 B = 45 KiB).
const DRAW_ROWS: usize = 48;

/// Base of the PSRAM shadow frame (`WIDTH * HEIGHT` RGB565 pixels).
static SHADOW: AtomicPtr<u16> = AtomicPtr::new(core::ptr::null_mut());
/// True while a [`ScreenLease`] exists.
static BUSY: AtomicBool = AtomicBool::new(false);
/// The page snapshot used by slides; set once by `DisplayPort::new`.
static SNAPSHOT: AtomicPtr<u16> = AtomicPtr::new(core::ptr::null_mut());
/// Set while a [`SnapshotLease`] exists.
static SNAPSHOT_BUSY: AtomicBool = AtomicBool::new(false);
/// Panel area flushed since the last [`DisplayPort::take_frame`].
static DAMAGE: Mutex<Cell<Option<DirtyRect>>> = Mutex::new(Cell::new(None));

static POINTER_X: AtomicI32 = AtomicI32::new(0);
static POINTER_Y: AtomicI32 = AtomicI32::new(0);
static POINTER_DOWN: AtomicBool = AtomicBool::new(false);
/// Set on every press; cleared once LVGL has read a pressed state.
static POINTER_LATCH: AtomicBool = AtomicBool::new(false);

/// Report the latest touch sample to LVGL's pointer device.
pub fn set_pointer(x: i32, y: i32, pressed: bool) {
    POINTER_X.store(x, Ordering::Relaxed);
    POINTER_Y.store(y, Ordering::Relaxed);
    if pressed {
        POINTER_LATCH.store(true, Ordering::Relaxed);
    }
    POINTER_DOWN.store(pressed, Ordering::Relaxed);
}

/// Read access to the shadow frame for one upload. Dropping it lets LVGL
/// render again.
pub struct ScreenLease {
    pixels: &'static [u16],
    #[cfg(feature = "native-ui-diagnostics")]
    profile_id: Option<u32>,
}

// SAFETY: the shadow is written only by LVGL's flush on the UI task, and the
// UI task does not run LVGL while a lease exists (`BUSY`).
unsafe impl Send for ScreenLease {}

impl ScreenLease {
    pub fn pixels(&self) -> &[u16] {
        self.pixels
    }
    #[cfg(feature = "native-ui-diagnostics")]
    pub fn profile_id(&self) -> Option<u32> {
        self.profile_id
    }
}

impl Drop for ScreenLease {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
        #[cfg(feature = "native-ui-diagnostics")]
        if let Some(id) = self.profile_id {
            let released_at = os::telemetry::latency::now_us();
            os::telemetry::native_ui::with(|p| p.release(id, released_at));
        }
    }
}

/// Read access to the page snapshot for one slide frame.
pub struct SnapshotLease {
    pixels: &'static [u16],
}

// SAFETY: the snapshot is written only by `DisplayPort::snapshot` on the UI
// task, which refuses while a lease exists (`SNAPSHOT_BUSY`).
unsafe impl Send for SnapshotLease {}

impl SnapshotLease {
    pub fn pixels(&self) -> &[u16] {
        self.pixels
    }
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        SNAPSHOT_BUSY.store(false, Ordering::Release);
    }
}

/// The full panel rectangle.
const FULL: DirtyRect = DirtyRect {
    x0: 0,
    y0: 0,
    x1: WIDTH as u16 - 1,
    y1: HEIGHT as u16 - 1,
};

/// The firmware's sole LVGL display, owned by the UI task.
pub struct DisplayPort {
    display: NonNull<sys::lv_display_t>,
    indev: NonNull<sys::lv_indev_t>,
    // C retains only these allocations' addresses, not Rust references.
    _draw: PixelMemory,
    _shadow: PixelMemory,
    _snapshot: PixelMemory,
}

impl DisplayPort {
    /// Register the LVGL display and pointer device.
    ///
    /// # Safety
    ///
    /// LVGL must already be initialized and every LVGL call must be serialized
    /// on one thread (the UI task). No native display/widget handle may outlive
    /// this value, and the process must not delete the display separately.
    /// Only one `DisplayPort` may exist.
    pub unsafe fn new() -> Self {
        let format = sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565;
        let stride = unsafe { sys::lv_draw_buf_width_to_stride(WIDTH as u32, format) } as usize;
        let draw = PixelMemory::new(stride * DRAW_ROWS, false);
        let shadow = PixelMemory::new(WIDTH * HEIGHT * 2, true);
        SHADOW.store(shadow.as_ptr().cast(), Ordering::Release);
        let snapshot = PixelMemory::new(WIDTH * HEIGHT * 2, true);
        SNAPSHOT.store(snapshot.as_ptr().cast(), Ordering::Release);

        let display = NonNull::new(unsafe { sys::lv_display_create(WIDTH as i32, HEIGHT as i32) })
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
            sys::lv_display_set_flush_cb(display.as_ptr(), Some(flush));
        }

        let indev = NonNull::new(unsafe { sys::lv_indev_create() })
            .expect("LVGL input device allocation failed");
        unsafe {
            sys::lv_indev_set_type(indev.as_ptr(), sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            sys::lv_indev_set_read_cb(indev.as_ptr(), Some(read_pointer));
            sys::lv_indev_set_display(indev.as_ptr(), display.as_ptr());
        }

        Self {
            display,
            indev,
            _draw: draw,
            _shadow: shadow,
            _snapshot: snapshot,
        }
    }

    /// True while the plane still reads the shadow; LVGL must not run.
    pub fn is_busy(&self) -> bool {
        BUSY.load(Ordering::Acquire)
    }

    // #region debug-point popup-shadow-stable-read
    /// Read only after the last damage has been submitted and its lease has
    /// completed. UI owner only; no await, LVGL call, or frame submission may
    /// occur while this borrow is used. A concurrent plane can only read pixels.
    #[cfg(feature = "native-ui-diagnostics")]
    pub fn diagnostic_stable_shadow(&self) -> Option<&[u16]> {
        if self.is_busy() || critical_section::with(|cs| DAMAGE.borrow(cs).get().is_some()) {
            return None;
        }
        // SAFETY: this display owns the allocation. The sole writer is the UI
        // task, which does not run LVGL or write_rect during the returned borrow.
        Some(unsafe { core::slice::from_raw_parts(self._shadow.as_ptr().cast(), WIDTH * HEIGHT) })
    }
    // #endregion debug-point popup-shadow-stable-read

    /// Report the successfully allocated configuration, outside rendering.
    #[cfg(feature = "native-ui-diagnostics")]
    pub fn report_draw_configuration(&self) {
        let stride = unsafe {
            sys::lv_draw_buf_width_to_stride(
                WIDTH as u32,
                sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
            )
        } as usize;
        let align = self._draw.alignment();
        log::info!(
            "native-buffer rows={} bytes={} stride={} align={} aligned={} caps=internal",
            self._draw.len() / stride,
            self._draw.len(),
            stride,
            align,
            u8::from(self._draw.as_ptr() as usize % align == 0),
        );
    }

    /// Mark the whole active screen for redraw (page switch).
    pub fn invalidate_all(&mut self) {
        unsafe { sys::lv_obj_invalidate(sys::lv_screen_active()) };
    }

    /// Hand the shadow and its damage to the plane, if anything was drawn and
    /// no upload is in flight.
    pub fn take_frame(&mut self) -> Option<(ScreenLease, DirtyRect)> {
        if self.is_busy() {
            return None;
        }
        let damage = critical_section::with(|cs| DAMAGE.borrow(cs).take())?;
        BUSY.store(true, Ordering::Release);
        #[cfg(feature = "native-ui-diagnostics")]
        let leased_at = os::telemetry::latency::now_us();
        let base = SHADOW.load(Ordering::Acquire);
        // SAFETY: `base` is the live `WIDTH * HEIGHT` shadow allocation owned
        // by `self._shadow`; LVGL does not write it while `BUSY` is set.
        let pixels = unsafe { core::slice::from_raw_parts(base, WIDTH * HEIGHT) };
        #[cfg(feature = "native-ui-diagnostics")]
        let profile_id = os::telemetry::native_ui::with(|p| {
            p.lease(leased_at, [damage.x0, damage.y0, damage.x1, damage.y1])
        });
        Some((
            ScreenLease {
                pixels,
                #[cfg(feature = "native-ui-diagnostics")]
                profile_id,
            },
            damage,
        ))
    }

    /// Copy the page on screen into the snapshot. Refuses (returns `false`)
    /// while the plane still reads either frame.
    pub fn snapshot(&mut self) -> bool {
        if self.is_busy() || SNAPSHOT_BUSY.load(Ordering::Acquire) {
            return false;
        }
        let shadow = SHADOW.load(Ordering::Acquire);
        let snapshot = SNAPSHOT.load(Ordering::Acquire);
        // SAFETY: both are live `WIDTH * HEIGHT` allocations owned by `self`;
        // neither is read by the plane (no lease) or written by LVGL (this
        // task) right now.
        unsafe { core::ptr::copy_nonoverlapping(shadow, snapshot, WIDTH * HEIGHT) };
        true
    }

    /// Render everything invalidated into the shadow right away, instead of
    /// waiting for LVGL's refresh timer. Must not be called while busy.
    pub fn render_now(&mut self) {
        debug_assert!(!self.is_busy());
        unsafe { sys::lv_refr_now(self.display.as_ptr()) };
    }

    /// Lease the shadow (the new page) and the snapshot (the old page) for
    /// one slide frame. The shadow's damage is kept for after the slide.
    pub fn take_slide(&mut self) -> Option<(ScreenLease, SnapshotLease)> {
        if self.is_busy() || SNAPSHOT_BUSY.load(Ordering::Acquire) {
            return None;
        }
        BUSY.store(true, Ordering::Release);
        SNAPSHOT_BUSY.store(true, Ordering::Release);
        // SAFETY: as in `take_frame`; the snapshot is not rewritten while the
        // lease exists.
        let screen =
            unsafe { core::slice::from_raw_parts(SHADOW.load(Ordering::Acquire), WIDTH * HEIGHT) };
        let old = unsafe {
            core::slice::from_raw_parts(SNAPSHOT.load(Ordering::Acquire), WIDTH * HEIGHT)
        };
        Some((
            ScreenLease {
                pixels: screen,
                #[cfg(feature = "native-ui-diagnostics")]
                profile_id: None,
            },
            SnapshotLease { pixels: old },
        ))
    }

    /// Hand the whole shadow to the plane (the last frame of a slide).
    pub fn take_full_frame(&mut self) -> Option<(ScreenLease, DirtyRect)> {
        if self.is_busy() {
            return None;
        }
        critical_section::with(|cs| DAMAGE.borrow(cs).set(None));
        BUSY.store(true, Ordering::Release);
        // SAFETY: as in `take_frame`.
        let pixels =
            unsafe { core::slice::from_raw_parts(SHADOW.load(Ordering::Acquire), WIDTH * HEIGHT) };
        Some((
            ScreenLease {
                pixels,
                #[cfg(feature = "native-ui-diagnostics")]
                profile_id: None,
            },
            FULL,
        ))
    }

    /// Forget a press LVGL has not seen yet (touches during a slide).
    pub fn reset_pointer(&mut self) {
        set_pointer(0, 0, false);
        POINTER_LATCH.store(false, Ordering::Relaxed);
        unsafe { sys::lv_indev_reset(self.indev.as_ptr(), core::ptr::null_mut()) };
    }

    /// Drop any damage collected so far (the plane shows the fox instead).
    pub fn discard_damage(&mut self) {
        critical_section::with(|cs| DAMAGE.borrow(cs).set(None));
    }
}

impl DisplayPort {
    /// Copy a rectangle of RGB565 pixels into the shadow and add it to the
    /// damage, as LVGL's flush does. Used by Lua apps, which draw into their
    /// own framebuffer. Refuses (returns `false`) while the plane reads the
    /// shadow. Bounds are exclusive and must lie inside the screen.
    ///
    /// # Safety
    ///
    /// `pixels` must point at `y1 - y0` rows of `(x1 - x0) * 2` readable
    /// bytes, `stride` bytes apart.
    pub unsafe fn write_rect(
        &mut self,
        (x0, y0, x1, y1): (u16, u16, u16, u16),
        pixels: *const u8,
        stride: usize,
    ) -> bool {
        if self.is_busy() {
            return false;
        }
        let (x0, y0) = (x0 as usize, y0 as usize);
        let (x1, y1) = ((x1 as usize).min(WIDTH), (y1 as usize).min(HEIGHT));
        if x0 >= x1 || y0 >= y1 || pixels.is_null() {
            return true;
        }
        let shadow = SHADOW.load(Ordering::Acquire);
        for row in 0..y1 - y0 {
            // SAFETY: the source row is readable per the caller's contract;
            // the destination row lies inside the `WIDTH * HEIGHT` shadow,
            // which the plane does not read (not busy). Byte copy: the
            // source has no alignment guarantee.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    pixels.add(row * stride),
                    shadow.add((y0 + row) * WIDTH + x0).cast::<u8>(),
                    (x1 - x0) * 2,
                );
            }
        }
        add_damage(DirtyRect {
            x0: x0 as u16,
            y0: y0 as u16,
            x1: (x1 - 1) as u16,
            y1: (y1 - 1) as u16,
        });
        true
    }
}

/// Grow the pending damage by `rect` (inclusive bounds).
fn add_damage(rect: DirtyRect) {
    critical_section::with(|cs| {
        let cell = DAMAGE.borrow(cs);
        cell.set(Some(match cell.get() {
            Some(old) => DirtyRect {
                x0: old.x0.min(rect.x0),
                y0: old.y0.min(rect.y0),
                x1: old.x1.max(rect.x1),
                y1: old.y1.max(rect.y1),
            },
            None => rect,
        }));
    });
}

impl Drop for DisplayPort {
    fn drop(&mut self) {
        // Remove C's callbacks before releasing the allocations.
        unsafe {
            sys::lv_indev_delete(self.indev.as_ptr());
            sys::lv_display_delete(self.display.as_ptr());
        }
        SHADOW.store(core::ptr::null_mut(), Ordering::Release);
        SNAPSHOT.store(core::ptr::null_mut(), Ordering::Release);
    }
}

unsafe extern "C" fn flush(
    display: *mut sys::lv_display_t,
    area: *const sys::lv_area_t,
    pixels: *mut u8,
) {
    #[cfg(feature = "native-ui-diagnostics")]
    let profile_start = os::telemetry::latency::now_us();
    #[cfg(feature = "native-ui-diagnostics")]
    let mut profile_pixels = 0u64;
    let shadow = SHADOW.load(Ordering::Acquire);
    if !shadow.is_null() && !area.is_null() && !pixels.is_null() {
        let area = unsafe { *area };
        let x1 = area.x1.clamp(0, WIDTH as i32 - 1);
        let x2 = area.x2.clamp(0, WIDTH as i32 - 1);
        let y1 = area.y1.clamp(0, HEIGHT as i32 - 1);
        let y2 = area.y2.clamp(0, HEIGHT as i32 - 1);
        if x1 <= x2 && y1 <= y2 {
            #[cfg(feature = "native-ui-diagnostics")]
            {
                profile_pixels = (x2 - x1 + 1) as u64 * (y2 - y1 + 1) as u64;
            }
            let area_w = (area.x2 - area.x1 + 1) as u32;
            let stride = unsafe {
                sys::lv_draw_buf_width_to_stride(
                    area_w,
                    sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
                )
            } as usize;
            let w = (x2 - x1 + 1) as usize;
            for y in y1..=y2 {
                let src_row = (y - area.y1) as usize * stride + (x1 - area.x1) as usize * 2;
                let dst = (y as usize * WIDTH + x1 as usize) as isize;
                // SAFETY: the source row lies inside LVGL's draw buffer for
                // `area`, the destination inside the shadow; byte copy avoids
                // any alignment assumption about the draw buffer.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        pixels.add(src_row),
                        shadow.offset(dst).cast::<u8>(),
                        w * 2,
                    );
                }
            }
            let rect = DirtyRect {
                x0: x1 as u16,
                y0: y1 as u16,
                x1: x2 as u16,
                y1: y2 as u16,
            };
            add_damage(rect);
        }
    }
    unsafe { sys::lv_display_flush_ready(display) };
    #[cfg(feature = "native-ui-diagnostics")]
    os::telemetry::native_ui::with(|p| {
        p.flush(
            os::telemetry::latency::now_us().saturating_sub(profile_start),
            profile_pixels,
        )
    });
}

unsafe extern "C" fn read_pointer(_indev: *mut sys::lv_indev_t, data: *mut sys::lv_indev_data_t) {
    if data.is_null() {
        return;
    }
    let latched = POINTER_LATCH.swap(false, Ordering::Relaxed);
    let pressed = POINTER_DOWN.load(Ordering::Relaxed) || latched;
    let data = unsafe { &mut *data };
    data.point.x = POINTER_X.load(Ordering::Relaxed);
    data.point.y = POINTER_Y.load(Ordering::Relaxed);
    data.state = if pressed {
        sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
}
