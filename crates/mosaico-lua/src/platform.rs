//! Services for the C modules (esp-claw display/json, delay, system, os):
//! time, large allocations, the display and touch. Called from
//! `csrc/mosaico_platform.c` and `csrc/mosaico_loslib.c`.
//!
//! These C entry points have no `lua_State`, so they reach the running
//! script's [`Host`] through "the current context", set by [`Vm`] while it
//! runs Lua code (resume, load, drop). One script runs at a time.
//!
//! [`Vm`]: crate::Vm

use core::{
    alloc::Layout,
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering},
};

use crate::vm::{Ctx, RawAllocator, GLOBAL_ALLOCATOR};

/// ESP-IDF error codes as seen by scripts (`esp_err_to_name`).
pub mod esp_err {
    pub const FAIL: i32 = -1;
    pub const NO_MEM: i32 = 0x101;
    pub const INVALID_ARG: i32 = 0x102;
    pub const INVALID_STATE: i32 = 0x103;
    pub const NOT_FOUND: i32 = 0x105;
    pub const NOT_SUPPORTED: i32 = 0x106;
    pub const TIMEOUT: i32 = 0x107;
}

/// Geometry of the screen a script draws on (RGB565 only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayInfo {
    pub width: u16,
    pub height: u16,
    pub touch_available: bool,
}

/// A rectangle of RGB565 pixels handed over by `display:present()`.
/// `pixels` starts at (x0, y0); rows are `stride` bytes apart.
pub struct Blit<'a> {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
    pub pixels: &'a [u8],
    pub stride: usize,
}

impl Blit<'_> {
    pub fn width(&self) -> usize {
        (self.x1 - self.x0) as usize
    }
    pub fn height(&self) -> usize {
        (self.y1 - self.y0) as usize
    }
    /// Pixel row `r` (0-based within the rectangle) as native-endian u16s.
    pub fn row(&self, r: usize) -> impl Iterator<Item = u16> + '_ {
        let start = r * self.stride;
        self.pixels[start..start + self.width() * 2]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchPoint {
    pub id: u8,
    pub x: i32,
    pub y: i32,
}

/// `system.heap.get_info` fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct HeapInfo {
    pub total_size: u32,
    pub free_size: u32,
    pub allocated_size: u32,
    pub minimum_free_size: u32,
    pub largest_free_block: u32,
    pub allocated_blocks: u32,
    pub free_blocks: u32,
    pub total_blocks: u32,
}

/// `heap_caps` flags as exposed in `system.heap.caps`.
pub mod caps {
    pub const SPIRAM: u32 = 1 << 10;
    pub const INTERNAL: u32 = 1 << 11;
    pub const DEFAULT: u32 = 1 << 12;
}

// ---- current context -------------------------------------------------------

#[cfg(not(test))]
static CURRENT: AtomicPtr<Ctx> = AtomicPtr::new(ptr::null_mut());

#[cfg(test)]
std::thread_local! {
    static CURRENT: core::cell::Cell<*mut Ctx> = const { core::cell::Cell::new(ptr::null_mut()) };
}

/// Makes `ctx` current until the guard drops (restores the previous one).
pub(crate) struct Enter(*mut Ctx);

impl Enter {
    pub(crate) fn new(ctx: *mut Ctx) -> Enter {
        Enter(swap_current(ctx))
    }
}

impl Drop for Enter {
    fn drop(&mut self) {
        swap_current(self.0);
    }
}

fn swap_current(ctx: *mut Ctx) -> *mut Ctx {
    #[cfg(not(test))]
    {
        CURRENT.swap(ctx, Ordering::AcqRel)
    }
    #[cfg(test)]
    {
        CURRENT.with(|c| c.replace(ctx))
    }
}

fn current() -> Option<&'static mut Ctx> {
    #[cfg(not(test))]
    let p = CURRENT.load(Ordering::Acquire);
    #[cfg(test)]
    let p = CURRENT.with(|c| c.get());
    unsafe { p.as_mut() }
}

// ---- allocation --------------------------------------------------------------

static ALLOCATOR: AtomicPtr<RawAllocator> = AtomicPtr::new(ptr::null_mut());
static PLATFORM_USED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn set_allocator(a: &'static RawAllocator) {
    ALLOCATOR.store(a as *const _ as *mut _, Ordering::Release);
}

fn allocator() -> &'static RawAllocator {
    unsafe { ALLOCATOR.load(Ordering::Acquire).as_ref() }.unwrap_or(&GLOBAL_ALLOCATOR)
}

/// Bytes held by C modules (framebuffers), outside the Lua heap cap.
pub fn platform_bytes() -> usize {
    PLATFORM_USED.load(Ordering::Relaxed)
}

const HEADER: usize = 16;

#[no_mangle]
unsafe extern "C" fn mosaico_host_alloc(size: usize, align: usize) -> *mut c_void {
    // [pad .. | size: usize, align: usize | user data], header right before data.
    let align = align.max(HEADER).next_power_of_two();
    let Some(total) = size.checked_add(align) else {
        return ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, align) else {
        return ptr::null_mut();
    };
    let base = (allocator().alloc)(layout);
    if base.is_null() {
        return ptr::null_mut();
    }
    let user = base.add(align);
    let hdr = user.sub(2 * core::mem::size_of::<usize>()).cast::<usize>();
    hdr.write(total);
    hdr.add(1).write(align);
    PLATFORM_USED.fetch_add(total, Ordering::Relaxed);
    user.cast()
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_free(user: *mut c_void) {
    if user.is_null() {
        return;
    }
    let user = user.cast::<u8>();
    let hdr = user.sub(2 * core::mem::size_of::<usize>()).cast::<usize>();
    let total = hdr.read();
    let align = hdr.add(1).read();
    PLATFORM_USED.fetch_sub(total, Ordering::Relaxed);
    (allocator().dealloc)(
        user.sub(align),
        Layout::from_size_align_unchecked(total, align),
    );
}

// ---- time --------------------------------------------------------------------

static FALLBACK_MS: AtomicU32 = AtomicU32::new(0);

/// Microseconds from the script's clock ([`crate::Limits::clock`]). Without
/// one, a counter that advances 1 ms per reading keeps busy-waits finite.
pub(crate) fn now_us() -> u64 {
    match current().and_then(|c| c.limits.clock) {
        Some(clock) => clock(),
        None => FALLBACK_MS.fetch_add(1, Ordering::Relaxed) as u64 * 1000,
    }
}

#[no_mangle]
extern "C" fn mosaico_host_now_us() -> i64 {
    now_us() as i64
}

#[no_mangle]
extern "C" fn mosaico_host_unix_us() -> i64 {
    match current().and_then(|c| c.host.unix_time_us()) {
        Some(t) => t,
        None => now_us() as i64,
    }
}

// ---- log ---------------------------------------------------------------------

unsafe fn c_str<'a>(s: *const c_char) -> &'a [u8] {
    if s.is_null() {
        return b"";
    }
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(s.cast(), n)
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_log(level: c_char, tag: *const c_char, msg: *const c_char) {
    let mut line = alloc::vec::Vec::new();
    line.push(level as u8);
    line.extend_from_slice(b" (");
    line.extend_from_slice(c_str(tag));
    line.extend_from_slice(b") ");
    line.extend_from_slice(c_str(msg));
    match current() {
        Some(ctx) => ctx.host.log(&line),
        None => crate::vm::log_line(&line),
    }
}

// ---- display -----------------------------------------------------------------

#[repr(C)]
struct CDisplayInfo {
    width: u16,
    height: u16,
    bits_per_pixel: u32,
    panel_interface: c_int,
    touch_available: bool,
}

#[repr(C)]
struct CTouchPoint {
    id: u8,
    x: i32,
    y: i32,
}

const MAX_TOUCH_POINTS: usize = 5;

#[repr(C)]
struct CTouchSnapshot {
    count: u8,
    points: [CTouchPoint; MAX_TOUCH_POINTS],
}

unsafe fn write_info(out: *mut CDisplayInfo, info: DisplayInfo) {
    out.write(CDisplayInfo {
        width: info.width,
        height: info.height,
        bits_per_pixel: 16,
        panel_interface: 0,
        touch_available: info.touch_available,
    });
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_display_open(out: *mut CDisplayInfo) -> c_int {
    let Some(ctx) = current() else {
        return esp_err::INVALID_STATE;
    };
    match ctx.host.display_open() {
        Ok(info) => {
            write_info(out, info);
            0
        }
        Err(e) => e,
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_display_info(out: *mut CDisplayInfo) -> c_int {
    let Some(ctx) = current() else {
        return esp_err::INVALID_STATE;
    };
    match ctx.host.display_info() {
        Some(info) => {
            write_info(out, info);
            0
        }
        None => esp_err::INVALID_STATE,
    }
}

#[no_mangle]
extern "C" fn mosaico_host_display_close() -> c_int {
    match current() {
        Some(ctx) => {
            ctx.host.display_close();
            0
        }
        None => esp_err::INVALID_STATE,
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_display_blit(
    x0: c_int,
    y0: c_int,
    x1: c_int,
    y1: c_int,
    fb: *const u8,
    stride: usize,
) -> c_int {
    let Some(ctx) = current() else {
        return esp_err::INVALID_STATE;
    };
    let Some(info) = ctx.host.display_info() else {
        return esp_err::INVALID_STATE;
    };
    let (w, h) = (info.width as c_int, info.height as c_int);
    if x0 < 0 || y0 < 0 || x1 > w || y1 > h || x0 >= x1 || y0 >= y1 || fb.is_null() {
        return esp_err::INVALID_ARG;
    }
    let row_bytes = (x1 - x0) as usize * 2;
    let stride = if stride == 0 { row_bytes } else { stride };
    let len = (y1 - y0 - 1) as usize * stride + row_bytes;
    let blit = Blit {
        x0: x0 as u16,
        y0: y0 as u16,
        x1: x1 as u16,
        y1: y1 as u16,
        pixels: core::slice::from_raw_parts(fb, len),
        stride,
    };
    match ctx.host.display_blit(&blit) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_display_touch(out: *mut CTouchSnapshot) -> c_int {
    let Some(ctx) = current() else {
        return esp_err::INVALID_STATE;
    };
    let mut points = [TouchPoint::default(); MAX_TOUCH_POINTS];
    match ctx.host.touch(&mut points) {
        Ok(n) => {
            let n = n.min(MAX_TOUCH_POINTS);
            let out = &mut *out;
            out.count = n as u8;
            for (dst, src) in out.points.iter_mut().zip(&points[..n]) {
                *dst = CTouchPoint {
                    id: src.id,
                    x: src.x,
                    y: src.y,
                };
            }
            0
        }
        Err(e) => e,
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_heap_info(caps: u32, out: *mut HeapInfo) {
    let info = current()
        .map(|c| c.host.heap_info(caps))
        .unwrap_or_default();
    out.write(info);
}

// ---- snapshot hardware -----------------------------------------------------
#[no_mangle]
unsafe extern "C" fn mosaico_host_hardware_snapshot(out: *mut crate::HardwareSnapshot) {
    let snapshot = current()
        .map(|c| c.host.hardware_snapshot())
        .unwrap_or_default();
    out.write(snapshot);
}

#[no_mangle]
unsafe extern "C" fn mosaico_host_hardware_request(kind: u32, data: *const u8) -> c_int {
    use crate::{HapticPreset, HardwareRequest};
    let Some(ctx) = current() else {
        return esp_err::INVALID_STATE;
    };
    let request = match kind {
        0 if !data.is_null() => {
            let Some(preset) = HapticPreset::ALL.get(*data as usize) else {
                return esp_err::INVALID_ARG;
            };
            HardwareRequest::Haptic(*preset)
        }
        1 if !data.is_null() => HardwareRequest::LedFrame(data.cast::<[[u8; 3]; 6]>().read()),
        2 => HardwareRequest::LedRelease,
        _ => return esp_err::INVALID_ARG,
    };
    match ctx.host.hardware_request(request) {
        Ok(()) => 0,
        Err(e) => e,
    }
}
