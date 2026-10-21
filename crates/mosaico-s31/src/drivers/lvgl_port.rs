//! Headless LVGL display registration.
//!
//! LVGL needs a registered display to own the screen object tree, but the
//! firmware never calls `lv_timer_handler`: the fox canvas is uploaded by
//! `plane_task` straight from the canvas pool. This port therefore keeps
//! only a one-row draw buffer and a flush callback that acknowledges
//! immediately, instead of the former 480x48 internal draw buffer and two
//! 480x480 PSRAM presentation frames.
//!
//! If LVGL rendering is ever reintroduced, it needs a real flush path again;
//! the callback below would silently drop pixels.

use core::ptr::NonNull;

use lvgl::sys;

use crate::drivers::memory::PixelMemory;

/// The firmware's sole LVGL display, owned by the UI task.
pub struct DisplayPort {
    display: NonNull<sys::lv_display_t>,
    // C retains only this allocation's address, not a Rust reference.
    _draw: PixelMemory,
}

impl DisplayPort {
    /// Register the headless LVGL display.
    ///
    /// # Safety
    ///
    /// LVGL must already be initialized and every LVGL call must be serialized
    /// on one thread (the UI task). No native display/widget handle may outlive
    /// this value, and the process must not delete the display separately.
    pub unsafe fn new() -> Self {
        let format = sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565;
        let stride =
            unsafe { sys::lv_draw_buf_width_to_stride(crate::drivers::geometry::WIDTH as u32, format) }
                as usize;
        let draw = PixelMemory::new(stride, false);

        let display = NonNull::new(unsafe {
            sys::lv_display_create(
                crate::drivers::geometry::WIDTH as i32,
                crate::drivers::geometry::HEIGHT as i32,
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
            sys::lv_display_set_flush_cb(display.as_ptr(), Some(flush));
        }

        Self {
            display,
            _draw: draw,
        }
    }

}

impl Drop for DisplayPort {
    fn drop(&mut self) {
        // Remove C's callback before releasing the draw allocation.
        unsafe { sys::lv_display_delete(self.display.as_ptr()) };
    }
}

unsafe extern "C" fn flush(
    display: *mut sys::lv_display_t,
    _area: *const sys::lv_area_t,
    _pixels: *mut u8,
) {
    unsafe { sys::lv_display_flush_ready(display) };
}
