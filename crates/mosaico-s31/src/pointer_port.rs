//! LVGL pointer input device fed from the UI task's latest touch state.
//!
//! The indirection exists because LVGL invokes the read callback from C while
//! rendering. The callback only copies the latest [`PointerReading`] prepared
//! by `ui_task`; it never drains a queue, touches hardware, or reenters the UI.
//!
//! Delivery is explicit: the indev runs in `LV_INDEV_MODE_EVENT`, its internal
//! read timer is paused after each read, and [`PointerPort::dispatch`] is the
//! only path that calls `lv_indev_read`. That keeps pointer processing on the
//! UI task's schedule and away from `lv_timer_handler`'s render pass.

use alloc::boxed::Box;
use core::{
    cell::{Cell, RefCell},
    ptr::NonNull,
};

use lvgl::sys;

use crate::{input_queue::CAPACITY, lvgl_port::DisplayPort};

/// One pointer sample prepared by `ui_task`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PointerReading {
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
    /// Monotonic milliseconds from the touch sample. `0` lets LVGL use its
    /// current tick, which is harmless for the first sample after boot.
    pub timestamp_ms: u32,
    /// More readings are queued in the same batch. R1 always reports `false`
    /// because `ui_task` already collapsed the input queue to its latest state.
    pub more: bool,
}

struct State {
    latest: RefCell<PointerReading>,
    callbacks: Cell<u32>,
    more: Cell<bool>,
}

/// The firmware's sole LVGL pointer indev.
///
/// The borrow of [`DisplayPort`] enforces that the indev is destroyed before
/// its display.
pub struct PointerPort<'display> {
    pointer: NonNull<sys::lv_indev_t>,
    state: Box<State>,
    _display: &'display DisplayPort,
}

impl<'display> PointerPort<'display> {
    /// Create a pointer indev for `display` in EVENT mode.
    ///
    /// # Safety
    ///
    /// LVGL must be initialized and all calls must be serialized on the same
    /// thread as the display. Native code must not delete this indev.
    pub unsafe fn new(display: &'display DisplayPort) -> Self {
        let state = Box::new(State {
            latest: RefCell::new(PointerReading::default()),
            callbacks: Cell::new(0),
            more: Cell::new(false),
        });
        let pointer = NonNull::new(unsafe { sys::lv_indev_create() })
            .expect("LVGL pointer allocation failed");
        unsafe {
            sys::lv_indev_set_type(pointer.as_ptr(), sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            sys::lv_indev_set_display(pointer.as_ptr(), display.as_raw());
            sys::lv_indev_set_user_data(
                pointer.as_ptr(),
                (&*state as *const State).cast_mut().cast(),
            );
            sys::lv_indev_set_read_cb(pointer.as_ptr(), Some(read_pointer));
            sys::lv_indev_set_mode(pointer.as_ptr(), sys::lv_indev_mode_t_LV_INDEV_MODE_EVENT);
            // EVENT mode still creates a read timer. Pause it immediately so
            // only `dispatch` can deliver input.
            let timer = sys::lv_indev_get_read_timer(pointer.as_ptr());
            if !timer.is_null() {
                sys::lv_timer_pause(timer);
            }
        }
        Self {
            pointer,
            state,
            _display: display,
        }
    }

    /// Replace the sample the next `lv_indev_read` will report.
    pub fn set_reading(&self, reading: PointerReading) {
        *self.state.latest.borrow_mut() = reading;
    }

    /// Deliver the latest sample once, exactly like the archived port.
    ///
    /// An empty batch still supplies the held state so LVGL's long-press clock
    /// advances. A disabled input or an animated screen transition may skip the
    /// read callback; the loop stops when no callback ran instead of blocking.
    /// Returns the number of delivered readings.
    pub fn dispatch(&self) -> usize {
        let mut delivered = 0;
        for _ in 0..CAPACITY {
            let before = self.state.callbacks.get();
            unsafe {
                sys::lv_indev_read(self.pointer.as_ptr());
                // LVGL 9.6 resumes this timer during a held press even in EVENT
                // mode. Pause it again: only ui_task may deliver input.
                let timer = sys::lv_indev_get_read_timer(self.pointer.as_ptr());
                if !timer.is_null() {
                    sys::lv_timer_pause(timer);
                }
            }
            if self.state.callbacks.get() == before {
                break;
            }
            delivered += 1;
            if !self.state.more.get() {
                break;
            }
        }
        delivered
    }
}

impl Drop for PointerPort<'_> {
    fn drop(&mut self) {
        // Unregister the callback and its timer before the state is released.
        unsafe { sys::lv_indev_delete(self.pointer.as_ptr()) };
    }
}

unsafe extern "C" fn read_pointer(pointer: *mut sys::lv_indev_t, data: *mut sys::lv_indev_data_t) {
    // SAFETY: `pointer` is the indev created by PointerPort and `user_data`
    // points at its live Box<State>.
    let state = unsafe { &*sys::lv_indev_get_user_data(pointer).cast::<State>() };
    let reading = *state.latest.borrow();
    let data = unsafe { &mut *data };
    data.point.x = reading.x;
    data.point.y = reading.y;
    data.state = if reading.pressed {
        sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
    data.timestamp = reading.timestamp_ms;
    // EVENT mode ignores continue_reading; dispatch() drains with its own
    // progress check.
    data.continue_reading = false;
    state.more.set(reading.more);
    state.callbacks.set(state.callbacks.get().wrapping_add(1));
}
