//! LVGL display and pointer registration for the hub pages (RFC 0006).
//!
//! The fox home page never goes through LVGL rendering: its canvas is
//! uploaded by `plane_task` straight from the canvas pool. The launcher and
//! the native apps are ordinary LVGL widget trees, rendered here:
//!
//! - LVGL renders in PARTIAL mode into a [`DRAW_ROWS`]-row draw buffer. The
//!   default dual-core RGB565 buffer uses PSRAM with full-frame capacity;
//!   RGB888 and single-core builds retain smaller internal-RAM buffers;
//! - the flush callback copies each band into a 480x480 PSRAM shadow frame
//!   and records bounded, separate damage regions;
//! - the UI task then hands the shadow to the plane as a [`ScreenLease`]
//!   with those regions. While the lease lives the shadow is "busy" and the
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
//! LVGL's configured read period is latched so taps are never lost.

use core::cell::Cell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

use super::damage::Damage;
use board_s31::pixels::{PixelFormat, PixelSource, PANEL_FORMAT};
use critical_section::Mutex;
use rs_lvgl::sys;

use crate::platform::runtime::memory::PixelMemory;
use {
    crate::services::display::geometry::DirtyRect, crate::services::display::geometry::HEIGHT,
    crate::services::display::geometry::WIDTH,
};

/// Dual-core RGB565 uses a 450 KiB PSRAM buffer in PARTIAL mode to avoid
/// repeating object/task preparation for each small band. RGB888 retains a
/// 33.75 KiB internal band; single-core retains 48 internal rows. The persistent
/// shadows use PSRAM, independently of this render buffer.
#[cfg(all(feature = "dual-core-ui", not(feature = "display-rgb888")))]
const DRAW_ROWS: usize = 480;
#[cfg(all(feature = "dual-core-ui", feature = "display-rgb888"))]
const DRAW_ROWS: usize = 24;
#[cfg(not(feature = "dual-core-ui"))]
const DRAW_ROWS: usize = 48;

/// Base of the PSRAM shadow frame (`FRAME_BYTES` panel-format bytes).
static SHADOW: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
/// True while a [`ScreenLease`] exists.
static BUSY: AtomicBool = AtomicBool::new(false);
/// The page snapshot used by slides; set once by `DisplayPort::new`.
static SNAPSHOT: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
/// Set while a [`SnapshotLease`] exists.
static SNAPSHOT_BUSY: AtomicBool = AtomicBool::new(false);
/// Panel area flushed since the last [`DisplayPort::take_frame`].
static DAMAGE: Mutex<Cell<Damage>> = Mutex::new(Cell::new(Damage::new()));

#[cfg(all(feature = "display-metrics", target_os = "none"))]
#[derive(Clone, Copy, Default)]
pub(crate) struct FlushStats {
    pub us: u32,
    pub calls: u32,
    pub pixels: u32,
}
#[cfg(all(feature = "display-metrics", target_os = "none"))]
static FLUSH_STATS: Mutex<Cell<FlushStats>> = Mutex::new(Cell::new(FlushStats {
    us: 0,
    calls: 0,
    pixels: 0,
}));

/// UI-thread diagnostics only. Flush remains synchronous into the same shadow.
#[cfg(all(feature = "display-metrics", target_os = "none"))]
pub(crate) fn take_flush_stats() -> FlushStats {
    critical_section::with(|cs| FLUSH_STATS.borrow(cs).take())
}

static POINTER_X: AtomicI32 = AtomicI32::new(0);
static POINTER_Y: AtomicI32 = AtomicI32::new(0);
static POINTER_DOWN: AtomicBool = AtomicBool::new(false);
static POINTER_PENDING: AtomicBool = AtomicBool::new(false);
/// Set on every press; cleared once LVGL has read a pressed state.
static POINTER_LATCH: AtomicBool = AtomicBool::new(false);

#[cfg(all(feature = "display-metrics", target_os = "none"))]
#[derive(Clone, Copy, Default)]
pub(crate) struct InputTrace {
    pub first: u64,
    pub last: u64,
    pub read: u64,
    pub count: u32,
}
#[cfg(all(feature = "display-metrics", target_os = "none"))]
static POINTER_TRACE: Mutex<Cell<InputTrace>> = Mutex::new(Cell::new(InputTrace {
    first: 0,
    last: 0,
    read: 0,
    count: 0,
}));
#[cfg(all(feature = "display-metrics", target_os = "none"))]
static READ_TRACE: Mutex<Cell<InputTrace>> = Mutex::new(Cell::new(InputTrace {
    first: 0,
    last: 0,
    read: 0,
    count: 0,
}));

/// Preserve the controller report timestamp before the UI's gesture clock clamp.
#[cfg(target_os = "none")]
pub fn set_pointer_at(x: i32, y: i32, pressed: bool, at_us: u64) {
    #[cfg(feature = "display-metrics")]
    critical_section::with(|cs| {
        let trace = POINTER_TRACE.borrow(cs);
        let mut value = trace.get();
        if value.count == 0 {
            value.first = at_us;
        }
        value.last = at_us;
        value.count = value.count.saturating_add(1);
        trace.set(value);
    });
    #[cfg(not(feature = "display-metrics"))]
    let _ = at_us;
    set_pointer(x, y, pressed);
}

/// Report the latest touch sample to LVGL's pointer device.
pub fn set_pointer(x: i32, y: i32, pressed: bool) {
    POINTER_X.store(x, Ordering::Relaxed);
    POINTER_Y.store(y, Ordering::Relaxed);
    if pressed {
        POINTER_LATCH.store(true, Ordering::Relaxed);
    }
    POINTER_DOWN.store(pressed, Ordering::Relaxed);
    POINTER_PENDING.store(true, Ordering::Relaxed);
}

/// Read access to the shadow frame for one upload. Dropping it lets LVGL
/// render again.
pub struct ScreenLease {
    pixels: &'static [u8],
    #[cfg(all(feature = "display-metrics", target_os = "none"))]
    acquired_us: u64,
    #[cfg(all(feature = "display-metrics", target_os = "none"))]
    pub(crate) input: InputTrace,
}

// SAFETY: the shadow is written only by LVGL's flush on the UI task, and the
// UI task does not run LVGL while a lease exists (`BUSY`).
unsafe impl Send for ScreenLease {}

impl ScreenLease {
    fn new(pixels: &'static [u8]) -> Self {
        Self {
            pixels,
            #[cfg(all(feature = "display-metrics", target_os = "none"))]
            acquired_us: crate::platform::runtime::time::now_us(),
            #[cfg(all(feature = "display-metrics", target_os = "none"))]
            input: critical_section::with(|cs| {
                READ_TRACE.borrow(cs).replace(InputTrace::default())
            }),
        }
    }
    pub fn pixels(&self) -> PixelSource<'_> {
        PixelSource::bytes(
            self.pixels,
            (WIDTH as u16, HEIGHT as u16),
            SHADOW_STRIDE,
            PANEL_FORMAT,
        )
    }
}

impl Drop for ScreenLease {
    fn drop(&mut self) {
        #[cfg(all(feature = "display-metrics", target_os = "none"))]
        log::info!(
            "display.busy begin={} end={}",
            self.acquired_us,
            crate::platform::runtime::time::now_us()
        );
        BUSY.store(false, Ordering::Release);
    }
}

/// Read access to the page snapshot for one slide frame.
pub struct SnapshotLease {
    pixels: &'static [u8],
}

// SAFETY: the snapshot is written only by `DisplayPort::snapshot` on the UI
// task, which refuses while a lease exists (`SNAPSHOT_BUSY`).
unsafe impl Send for SnapshotLease {}

impl SnapshotLease {
    pub fn pixels(&self) -> PixelSource<'_> {
        PixelSource::bytes(
            self.pixels,
            (WIDTH as u16, HEIGHT as u16),
            SHADOW_STRIDE,
            PANEL_FORMAT,
        )
    }
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        SNAPSHOT_BUSY.store(false, Ordering::Release);
    }
}

const SHADOW_STRIDE: usize = WIDTH * PANEL_FORMAT.bytes_per_pixel();
const FRAME_BYTES: usize = SHADOW_STRIDE * HEIGHT;
#[cfg(feature = "display-rgb888")]
const LVGL_FORMAT: sys::lv_color_format_t = sys::lv_color_format_t_LV_COLOR_FORMAT_RGB888;
#[cfg(not(feature = "display-rgb888"))]
const LVGL_FORMAT: sys::lv_color_format_t = sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565;
#[cfg(feature = "display-rgb888")]
const LVGL_STORAGE: PixelFormat = PixelFormat::Bgr888;
#[cfg(not(feature = "display-rgb888"))]
const LVGL_STORAGE: PixelFormat = PixelFormat::Rgb565Le;

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
    /// Every screen/snapshot lease must be dropped before this value is dropped.
    pub unsafe fn new() -> Self {
        critical_section::with(|cs| DAMAGE.borrow(cs).set(Damage::new()));
        POINTER_DOWN.store(false, Ordering::Relaxed);
        POINTER_LATCH.store(false, Ordering::Relaxed);
        POINTER_PENDING.store(false, Ordering::Relaxed);
        let format = LVGL_FORMAT;
        let stride = unsafe { sys::lv_draw_buf_width_to_stride(WIDTH as u32, format) } as usize;
        let draw = PixelMemory::new(
            stride * DRAW_ROWS,
            cfg!(all(
                feature = "dual-core-ui",
                not(feature = "display-rgb888")
            )),
        );
        let shadow = PixelMemory::new(FRAME_BYTES, true);
        SHADOW.store(shadow.as_ptr().cast(), Ordering::Release);
        let snapshot = PixelMemory::new(FRAME_BYTES, true);
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
            sys::lv_timer_set_period(sys::lv_display_get_refr_timer(display.as_ptr()), 16);
            #[cfg(all(feature = "display-metrics", target_os = "none"))]
            super::refresh_metrics::register(display.as_ptr());
        }

        let indev = NonNull::new(unsafe { sys::lv_indev_create() })
            .expect("LVGL input device allocation failed");
        unsafe {
            sys::lv_indev_set_type(indev.as_ptr(), sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            sys::lv_indev_set_read_cb(indev.as_ptr(), Some(read_pointer));
            sys::lv_indev_set_display(indev.as_ptr(), display.as_ptr());
            sys::lv_timer_set_period(sys::lv_indev_get_read_timer(indev.as_ptr()), 16);
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

    /// Read a newly queued sample on the serialized UI task, without waiting
    /// for the indev timer. Rendering remains paused while the shadow is leased.
    pub fn poll_pointer(&mut self) {
        if !self.is_busy() && POINTER_PENDING.swap(false, Ordering::Relaxed) {
            unsafe { sys::lv_indev_read(self.indev.as_ptr()) };
        }
    }

    /// Mark the whole active screen for redraw (page switch).
    pub fn invalidate_all(&mut self) {
        unsafe { sys::lv_obj_invalidate(sys::lv_screen_active()) };
    }

    /// Hand the shadow and its damage to the plane, if anything was drawn and
    /// no upload is in flight.
    pub fn take_frame(&mut self) -> Option<(ScreenLease, Damage)> {
        if self.is_busy() {
            return None;
        }
        let damage = critical_section::with(|cs| DAMAGE.borrow(cs).replace(Damage::new()));
        if damage.is_empty() {
            return None;
        }
        BUSY.store(true, Ordering::Release);

        let base = SHADOW.load(Ordering::Acquire);
        // SAFETY: `base` is the live `FRAME_BYTES` shadow allocation owned
        // by `self._shadow`; LVGL does not write it while `BUSY` is set.
        let pixels = unsafe { core::slice::from_raw_parts(base, FRAME_BYTES) };

        Some((ScreenLease::new(pixels), damage))
    }

    /// Copy the page on screen into the snapshot. Refuses (returns `false`)
    /// while the plane still reads either frame.
    pub fn snapshot(&mut self) -> bool {
        if self.is_busy() || SNAPSHOT_BUSY.load(Ordering::Acquire) {
            return false;
        }
        let shadow = SHADOW.load(Ordering::Acquire);
        let snapshot = SNAPSHOT.load(Ordering::Acquire);
        // SAFETY: both are live `FRAME_BYTES` allocations owned by `self`;
        // neither is read by the plane (no lease) or written by LVGL (this
        // task) right now.
        unsafe { core::ptr::copy_nonoverlapping(shadow, snapshot, FRAME_BYTES) };
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
            unsafe { core::slice::from_raw_parts(SHADOW.load(Ordering::Acquire), FRAME_BYTES) };
        let old =
            unsafe { core::slice::from_raw_parts(SNAPSHOT.load(Ordering::Acquire), FRAME_BYTES) };
        Some((ScreenLease::new(screen), SnapshotLease { pixels: old }))
    }

    /// Hand the whole shadow to the plane (the last frame of a slide).
    pub fn take_full_frame(&mut self) -> Option<(ScreenLease, Damage)> {
        if self.is_busy() {
            return None;
        }
        critical_section::with(|cs| DAMAGE.borrow(cs).set(Damage::new()));
        BUSY.store(true, Ordering::Release);
        // SAFETY: as in `take_frame`.
        let pixels =
            unsafe { core::slice::from_raw_parts(SHADOW.load(Ordering::Acquire), FRAME_BYTES) };
        Some((ScreenLease::new(pixels), Damage::full()))
    }

    /// Forget a press LVGL has not seen yet (touches during a slide).
    pub fn reset_pointer(&mut self) {
        set_pointer(0, 0, false);
        POINTER_LATCH.store(false, Ordering::Relaxed);
        unsafe { sys::lv_indev_reset(self.indev.as_ptr(), core::ptr::null_mut()) };
    }

    /// Drop any damage collected so far (the plane shows the fox instead).
    pub fn discard_damage(&mut self) {
        critical_section::with(|cs| DAMAGE.borrow(cs).set(Damage::new()));
    }
}

impl DisplayPort {
    /// Encode a rectangle of little-endian RGB565 pixels into the shadow and add it to the
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
        let length = (y1 - y0 - 1) * stride + (x1 - x0) * 2;
        // SAFETY: the caller supplies these initialized RGB565 rows until return.
        let bytes = unsafe { core::slice::from_raw_parts(pixels, length) };
        let source = PixelSource::bytes(
            bytes,
            ((x1 - x0) as u16, (y1 - y0) as u16),
            stride,
            PixelFormat::Rgb565Le,
        );
        copy_to_shadow(
            source,
            (x0, y0),
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: (x1 - x0 - 1) as u16,
                y1: (y1 - y0 - 1) as u16,
            },
        );
        add_damage(DirtyRect {
            x0: x0 as u16,
            y0: y0 as u16,
            x1: (x1 - 1) as u16,
            y1: (y1 - 1) as u16,
        });
        true
    }
}

/// Encode source rows once at the LVGL/Lua boundary into panel-ready storage.
fn copy_to_shadow(source: PixelSource<'_>, at: (usize, usize), area: DirtyRect) {
    debug_assert!(!BUSY.load(Ordering::Acquire));
    let shadow = SHADOW.load(Ordering::Acquire);
    let row_bytes = area.width() * PANEL_FORMAT.bytes_per_pixel();
    for row in 0..area.height() {
        let offset = (at.1 + row) * SHADOW_STRIDE + at.0 * PANEL_FORMAT.bytes_per_pixel();
        // SAFETY: UI-only copy, no live lease, destination is inside the frame.
        let output = unsafe { core::slice::from_raw_parts_mut(shadow.add(offset), row_bytes) };
        source.pack(
            DirtyRect {
                y0: area.y0 + row as u16,
                y1: area.y0 + row as u16,
                ..area
            },
            PANEL_FORMAT,
            output,
        );
    }
}

fn add_damage(rect: DirtyRect) {
    critical_section::with(|cs| {
        let cell = DAMAGE.borrow(cs);
        let mut damage = cell.get();
        damage.add(rect);
        cell.set(damage);
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
    #[cfg(all(feature = "display-metrics", target_os = "none"))]
    let started = crate::platform::runtime::time::now_us();
    #[cfg(all(feature = "display-metrics", target_os = "none"))]
    let mut flushed_pixels = 0;
    let shadow = SHADOW.load(Ordering::Acquire);
    if !shadow.is_null() && !area.is_null() && !pixels.is_null() {
        let area = unsafe { *area };
        let x1 = area.x1.clamp(0, WIDTH as i32 - 1);
        let x2 = area.x2.clamp(0, WIDTH as i32 - 1);
        let y1 = area.y1.clamp(0, HEIGHT as i32 - 1);
        let y2 = area.y2.clamp(0, HEIGHT as i32 - 1);
        if x1 <= x2 && y1 <= y2 {
            // PARTIAL buffers are reshaped by LVGL for each flush. Read the
            // actual native stride rather than infer it from display width.
            let buffer = unsafe { sys::lv_display_get_buf_active(display) };
            let stride = unsafe { (*buffer).header.stride() } as usize;
            let width = (area.x2 - area.x1 + 1) as usize;
            let height = (area.y2 - area.y1 + 1) as usize;
            let length = (height - 1) * stride + width * LVGL_STORAGE.bytes_per_pixel();
            // SAFETY: native active PARTIAL storage covers this flush; no view
            // escapes the callback or survives flush_ready.
            let bytes = unsafe { core::slice::from_raw_parts(pixels, length) };
            let source =
                PixelSource::bytes(bytes, (width as u16, height as u16), stride, LVGL_STORAGE);
            copy_to_shadow(
                source,
                (x1 as usize, y1 as usize),
                DirtyRect {
                    x0: (x1 - area.x1) as u16,
                    y0: (y1 - area.y1) as u16,
                    x1: (x2 - area.x1) as u16,
                    y1: (y2 - area.y1) as u16,
                },
            );
            let rect = DirtyRect {
                x0: x1 as u16,
                y0: y1 as u16,
                x1: x2 as u16,
                y1: y2 as u16,
            };
            add_damage(rect);
            #[cfg(all(feature = "display-metrics", target_os = "none"))]
            {
                flushed_pixels = rect.width() as u32 * rect.height() as u32;
            }
        }
    }
    unsafe { sys::lv_display_flush_ready(display) };
    #[cfg(all(feature = "display-metrics", target_os = "none"))]
    critical_section::with(|cs| {
        let cell = FLUSH_STATS.borrow(cs);
        let mut stats = cell.get();
        stats.us += crate::platform::runtime::time::now_us().saturating_sub(started) as u32;
        stats.calls += 1;
        stats.pixels += flushed_pixels;
        cell.set(stats);
    });
}

unsafe extern "C" fn read_pointer(_indev: *mut sys::lv_indev_t, data: *mut sys::lv_indev_data_t) {
    if data.is_null() {
        return;
    }
    #[cfg(all(feature = "display-metrics", target_os = "none"))]
    critical_section::with(|cs| {
        let mut trace = POINTER_TRACE.borrow(cs).replace(InputTrace::default());
        if trace.count != 0 {
            trace.read = crate::platform::runtime::time::now_us();
            READ_TRACE.borrow(cs).set(trace);
        }
    });
    let latched = POINTER_LATCH.swap(false, Ordering::Relaxed);
    let down = POINTER_DOWN.load(Ordering::Relaxed);
    let pressed = down || latched;
    // A complete tap may arrive between UI loops. Deliver its press now and
    // schedule its release on the next loop, even with no further input.
    POINTER_PENDING.store(latched && !down, Ordering::Relaxed);
    let data = unsafe { &mut *data };
    data.point.x = POINTER_X.load(Ordering::Relaxed);
    data.point.y = POINTER_Y.load(Ordering::Relaxed);
    data.state = if pressed {
        sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::display::{damage::FULL, simulator::MemoryPanel};

    fn native(run: impl FnOnce(&mut DisplayPort)) {
        let _lock = crate::test_support::lock();
        rs_lvgl::init();
        struct Shutdown;
        impl Drop for Shutdown {
            fn drop(&mut self) {
                unsafe { rs_lvgl::deinit() }
            }
        }
        let _shutdown = Shutdown;
        let mut port = unsafe { DisplayPort::new() };
        run(&mut port);
    }

    fn color(red: u8, green: u8, blue: u8) -> sys::lv_color_t {
        sys::lv_color_t { red, green, blue }
    }

    #[test]
    fn partial_buffer_capacity_and_reshaped_edge_updates_preserve_every_pixel() {
        native(|port| {
            let display = unsafe { sys::lv_display_get_default() };
            assert_eq!(
                unsafe { sys::lv_display_get_render_mode(display) },
                sys::lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL
            );
            let buffer = unsafe { &*sys::lv_display_get_buf_active(display) };
            let rows = if !cfg!(feature = "dual-core-ui") {
                48
            } else if cfg!(feature = "display-rgb888") {
                24
            } else {
                480
            };
            let capacity = WIDTH * rows * LVGL_STORAGE.bytes_per_pixel();
            assert_eq!(buffer.data_size as usize, capacity);
            assert_eq!(
                unsafe { sys::lv_display_get_color_format(display) },
                LVGL_FORMAT
            );
            println!(
                "partial-buffer dual_core={} rgb888={} bytes={} rows={} mode=PARTIAL",
                cfg!(feature = "dual-core-ui"),
                cfg!(feature = "display-rgb888"),
                buffer.data_size,
                rows
            );

            let screen = unsafe { sys::lv_screen_active() };
            unsafe {
                sys::lv_obj_remove_style_all(screen);
                sys::lv_obj_remove_flag(screen, sys::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
                sys::lv_obj_set_style_bg_color(screen, color(0, 0, 0), 0);
                sys::lv_obj_set_style_bg_opa(screen, 255, 0);
            }
            // Existing objects avoid creation invalidating their initial
            // geometry. Each later color change invalidates only this shape.
            let rects = [(37, 53, 1, 9), (93, 141, 13, 1), (479, 479, 1, 1)];
            let objects = rects.map(|(x, y, w, h)| unsafe {
                let obj = sys::lv_obj_create(screen);
                sys::lv_obj_remove_style_all(obj);
                sys::lv_obj_set_pos(obj, x, y);
                sys::lv_obj_set_size(obj, w, h);
                sys::lv_obj_set_style_bg_color(obj, color(0, 0, 0), 0);
                sys::lv_obj_set_style_bg_opa(obj, 255, 0);
                obj
            });
            let mut expected = [[0, 0, 0]; 3];
            let mut panel = MemoryPanel::new((480, 480), PANEL_FORMAT);
            let mut present = |port: &mut DisplayPort, background, expected: &[[u8; 3]; 3]| {
                port.render_now();
                let (lease, damage) = port.take_frame().unwrap();
                for rect in damage.regions() {
                    panel.upload(*rect, *rect, lease.pixels());
                }
                let source = lease.pixels();
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        let mut pixel = background;
                        for (i, &(left, top, w, h)) in rects.iter().enumerate() {
                            if x >= left as usize
                                && x < (left + w) as usize
                                && y >= top as usize
                                && y < (top + h) as usize
                            {
                                pixel = expected[i];
                            }
                        }
                        assert_eq!(source.rgb888_at(x, y), pixel, "shadow ({x},{y})");
                        assert_eq!(panel.pixels().rgb888_at(x, y), pixel, "panel ({x},{y})");
                    }
                }
                assert!(port.is_busy());
                assert!(port.take_frame().is_none());
                let regions = damage.regions().len();
                drop(lease);
                assert!(!port.is_busy());
                regions
            };
            present(port, [0, 0, 0], &expected);
            for (i, pixel) in [[255, 0, 0], [0, 255, 0], [0, 0, 255]]
                .into_iter()
                .enumerate()
            {
                expected[i] = pixel;
                unsafe {
                    sys::lv_obj_set_style_bg_color(
                        objects[i],
                        color(pixel[0], pixel[1], pixel[2]),
                        0,
                    );
                }
                present(port, [0, 0, 0], &expected);
                let buffer = unsafe { &*sys::lv_display_get_buf_active(display) };
                println!(
                    "partial-edge case={} w={} h={} stride={} bytes={}",
                    i,
                    buffer.header.w(),
                    buffer.header.h(),
                    buffer.header.stride(),
                    buffer.data_size
                );
            }
            unsafe { sys::lv_obj_set_style_bg_color(screen, color(255, 255, 255), 0) };
            present(port, [255, 255, 255], &expected);
            expected[0] = [255, 0, 255];
            expected[2] = [0, 255, 255];
            for i in [0, 2] {
                let pixel = expected[i];
                unsafe {
                    sys::lv_obj_set_style_bg_color(
                        objects[i],
                        color(pixel[0], pixel[1], pixel[2]),
                        0,
                    );
                }
            }
            assert!(present(port, [255, 255, 255], &expected) >= 2);
            port.render_now();
            assert!(port.take_frame().is_none());
        });
    }

    #[test]
    fn native_flush_keeps_rgb888_precision_and_lease_blocks_writes_until_completion() {
        native(|port| {
            let mut panel = MemoryPanel::new((480, 480), PANEL_FORMAT);
            unsafe {
                sys::lv_obj_set_style_bg_color(sys::lv_screen_active(), color(0x13, 0x57, 0x9b), 0);
                sys::lv_obj_set_style_bg_opa(sys::lv_screen_active(), 255, 0);
            }
            port.render_now();
            let (lease, damage) = port.take_frame().unwrap();
            assert!(damage.is_full());
            panel.upload(FULL, FULL, lease.pixels());
            let actual = lease.pixels().rgb888_at(240, 240);
            if cfg!(feature = "display-rgb888") {
                assert_eq!(actual, [0x13, 0x57, 0x9b]);
            } else {
                assert_eq!(lease.pixels().rgb565_at(240, 240), 0x12b3);
            }
            assert!(port.is_busy());
            assert!(port.take_frame().is_none());
            assert!(port.take_slide().is_none());
            assert!(!port.snapshot());
            let pixel = 0xf800u16.to_le_bytes();
            assert!(!unsafe { port.write_rect((0, 0, 1, 1), pixel.as_ptr(), 2) });
            assert_eq!(lease.pixels().rgb888_at(240, 240), actual);
            drop(lease); // Simulated late upload completion.
            assert!(!port.is_busy());
            assert!(unsafe { port.write_rect((0, 0, 1, 1), pixel.as_ptr(), 2) });
            let (lease, damage) = port.take_frame().unwrap();
            assert_eq!(damage.pixels(), 8);
            assert_eq!(lease.pixels().rgb888_at(0, 0), [255, 0, 0]);
            // The expanded neighbouring row comes from unchanged shadow context.
            assert_eq!(lease.pixels().rgb888_at(0, 1), actual);
            for region in damage.regions() {
                assert!(region.is_panel_window());
                panel.upload(*region, *region, lease.pixels());
            }
            assert!(panel.matches(lease.pixels()));
            assert_eq!(lease.pixels().rgb888_at(240, 240), actual);
        });
    }

    #[test]
    fn separated_native_controls_upload_locally_and_unchanged_refresh_submits_nothing() {
        native(|port| {
            let make = |x, y| unsafe {
                let obj = sys::lv_obj_create(sys::lv_screen_active());
                sys::lv_obj_set_pos(obj, x, y);
                sys::lv_obj_set_size(obj, 40, 20);
                sys::lv_obj_set_style_radius(obj, 0, 0);
                sys::lv_obj_set_style_border_width(obj, 0, 0);
                sys::lv_obj_set_style_shadow_width(obj, 0, 0);
                sys::lv_obj_set_style_bg_opa(obj, 255, 0);
                obj
            };
            let first = make(20, 40);
            let second = make(400, 420);
            let mut panel = MemoryPanel::new((480, 480), PANEL_FORMAT);
            port.render_now();
            let (lease, _) = port.take_frame().unwrap();
            panel.upload(FULL, FULL, lease.pixels());
            drop(lease);
            port.render_now();
            assert!(port.take_frame().is_none());
            unsafe {
                sys::lv_obj_set_style_bg_color(first, color(0x13, 0x57, 0x9b), 0);
                sys::lv_obj_set_style_bg_color(second, color(0x91, 0x43, 0x27), 0);
            }
            port.render_now();
            let (lease, damage) = port.take_frame().unwrap();
            assert!(damage.regions().len() >= 2, "{damage:?}");
            assert!(damage.pixels() < WIDTH * HEIGHT / 10, "{damage:?}");
            for rect in damage.regions() {
                panel.upload(*rect, *rect, lease.pixels());
            }
            let source = lease.pixels();
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    assert_eq!(
                        panel.pixels().rgb888_at(x, y),
                        source.rgb888_at(x, y),
                        "({x},{y})"
                    );
                }
            }
            if cfg!(feature = "display-rgb888") {
                assert_eq!(panel.pixels().rgb888_at(30, 50), [0x13, 0x57, 0x9b]);
            }
        });
    }

    #[test]
    fn snapshot_and_slide_keep_independent_encoded_frames() {
        native(|port| {
            unsafe {
                sys::lv_obj_set_style_bg_color(sys::lv_screen_active(), color(255, 0, 0), 0);
            }
            port.render_now();
            drop(port.take_frame().unwrap());
            assert!(port.snapshot());
            unsafe {
                sys::lv_obj_set_style_bg_color(sys::lv_screen_active(), color(0, 0, 255), 0);
            }
            port.render_now();
            let (new, old) = port.take_slide().unwrap();
            assert_eq!(new.pixels().rgb888_at(240, 240), [0, 0, 255]);
            assert_eq!(old.pixels().rgb888_at(240, 240), [255, 0, 0]);
            assert!(!port.snapshot());
            drop(new);
            assert!(!port.snapshot());
            drop(old);
            assert!(port.snapshot());
            let (_, damage) = port.take_full_frame().unwrap();
            assert!(damage.is_full());
        });
    }

    #[test]
    fn a_tap_between_ui_loops_delivers_press_then_release() {
        native(|port| {
            set_pointer(100, 100, true);
            set_pointer(100, 100, false);
            port.poll_pointer();
            assert_eq!(
                unsafe { sys::lv_indev_get_state(port.indev.as_ptr()) },
                sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
            );
            port.poll_pointer();
            assert_eq!(
                unsafe { sys::lv_indev_get_state(port.indev.as_ptr()) },
                sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
            );
            assert!(!POINTER_PENDING.load(Ordering::Relaxed));
        });
    }
}
