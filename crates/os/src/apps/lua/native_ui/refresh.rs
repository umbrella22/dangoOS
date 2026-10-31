//! Publication boundaries and refresh/pointer timer guards.
use super::LuaLvglService;
use alloc::{boxed::Box, vec::Vec};
use core::{cell::Cell, ptr};
use lua_engine::ui::UiCommand;
use rs_lvgl::sys as s;
/// Tracks successful foreground UI requests so a delay-only script publishes
/// its rebuilt tree before sleeping. Shared by the device runner and host regression tests.
#[derive(Default)]
pub struct RedrawBoundary(bool);
impl RedrawBoundary {
    pub fn completed(&mut self, op: &str, success: bool) {
        if success {
            match op {
                "clean" => self.0 = true,
                "frame_end" | "deinit" => self.0 = false,
                _ => {}
            }
        }
    }
    pub fn before_sleep(&mut self, root: &str) -> Option<UiCommand> {
        if !core::mem::take(&mut self.0) {
            return None;
        }
        Some(UiCommand {
            op: "frame_end".into(),
            args: "[]".into(),
            bytes: Vec::new(),
            root: root.into(),
        })
    }
}
/// Limits native RPC work inside the UI task's existing idle interval. The
/// deadline is fixed at creation; accepting more work can never extend it.
pub struct RpcWindow {
    deadline_us: u64,
    remaining: u8,
}
impl RpcWindow {
    pub fn until(deadline_us: u64) -> Self {
        Self {
            deadline_us,
            remaining: 8,
        }
    }
    pub fn available(&self, now_us: u64) -> bool {
        self.remaining > 0 && now_us < self.deadline_us
    }
    pub fn completed(&mut self) {
        self.remaining = self.remaining.saturating_sub(1);
    }
}

pub(super) struct RefreshGate {
    display: *mut s::lv_display_t,
    timer: *mut s::lv_timer_t,
    pointers: Vec<Box<PointerPause>>,
}

struct PointerPause {
    indev: *mut s::lv_indev_t,
    timer: *mut s::lv_timer_t,
    alive: Cell<bool>,
}

unsafe extern "C" fn pointer_deleted(e: *mut s::lv_event_t) {
    unsafe {
        let pointer = &*(s::lv_event_get_user_data(e) as *const PointerPause);
        pointer.alive.set(false);
    }
}

impl RefreshGate {
    unsafe fn pause_pointers(display: *mut s::lv_display_t) -> Vec<Box<PointerPause>> {
        let mut pointers = Vec::new();
        unsafe {
            let mut indev = s::lv_indev_get_next(ptr::null_mut());
            while !indev.is_null() {
                if s::lv_indev_get_display(indev) == display
                    && s::lv_indev_get_type(indev) == s::lv_indev_type_t_LV_INDEV_TYPE_POINTER
                {
                    let timer = s::lv_indev_get_read_timer(indev);
                    // Leave pre-existing paused timers and every enabled bit
                    // untouched. Raw touch/edge gestures run outside LVGL.
                    if !timer.is_null() && !s::lv_timer_get_paused(timer) {
                        s::lv_timer_pause(timer);
                        s::lv_indev_reset(indev, ptr::null_mut());
                        let mut pointer = Box::new(PointerPause {
                            indev,
                            timer,
                            alive: Cell::new(true),
                        });
                        s::lv_indev_add_event_cb(
                            indev,
                            Some(pointer_deleted),
                            s::lv_event_code_t_LV_EVENT_DELETE,
                            (&mut *pointer as *mut PointerPause).cast(),
                        );
                        pointers.push(pointer);
                    }
                }
                indev = s::lv_indev_get_next(indev);
            }
        }
        pointers
    }
}

impl Drop for RefreshGate {
    fn drop(&mut self) {
        unsafe {
            for pointer in &self.pointers {
                if !pointer.alive.get() {
                    continue;
                }
                // A host may remove/reassign an input device while a redraw
                // is held. Find it without dereferencing the saved pointer.
                let mut live = s::lv_indev_get_next(ptr::null_mut());
                while !live.is_null() && live != pointer.indev {
                    live = s::lv_indev_get_next(live);
                }
                if !live.is_null() {
                    s::lv_indev_remove_event_cb_with_user_data(
                        live,
                        Some(pointer_deleted),
                        (&**pointer as *const PointerPause).cast_mut().cast(),
                    );
                }
                if live.is_null()
                    || s::lv_indev_get_display(live) != self.display
                    || s::lv_indev_get_type(live) != s::lv_indev_type_t_LV_INDEV_TYPE_POINTER
                    || s::lv_indev_get_read_timer(live) != pointer.timer
                {
                    continue;
                }
                s::lv_indev_reset(live, ptr::null_mut());
                s::lv_indev_wait_release(live);
                // The port latches a short tap as one PRESSED read even when
                // the finger is already up. Consume that read while guarded,
                // then read RELEASED to admit the first genuinely new tap.
                // A held contact remains PRESSED twice and must release.
                // Disabled devices stay disabled: lv_indev_read is a no-op.
                s::lv_indev_read(live);
                s::lv_indev_read(live);
                s::lv_timer_resume(pointer.timer);
            }
        }
    }
}

unsafe extern "C" fn hold_refresh(e: *mut s::lv_event_t) {
    unsafe {
        let gate = &*(s::lv_event_get_user_data(e) as *const RefreshGate);
        s::lv_timer_pause(gate.timer);
    }
}
impl LuaLvglService {
    pub(super) fn begin_redraw(&mut self) {
        if self.redraw.is_none() {
            unsafe {
                let display = s::lv_display_get_default();
                let timer = s::lv_display_get_refr_timer(display);
                if !timer.is_null() {
                    let mut gate = Box::new(RefreshGate {
                        display,
                        timer,
                        pointers: RefreshGate::pause_pointers(display),
                    });
                    s::lv_display_add_event_cb(
                        display,
                        Some(hold_refresh),
                        s::lv_event_code_t_LV_EVENT_REFR_REQUEST,
                        (&mut *gate as *mut RefreshGate).cast(),
                    );
                    s::lv_timer_pause(timer);
                    self.redraw = Some(gate);
                }
            }
        }
    }
    pub(super) fn finish_redraw(&mut self) {
        if let Some(mut gate) = self.redraw.take() {
            unsafe {
                s::lv_display_remove_event_cb_with_user_data(
                    gate.display,
                    Some(hold_refresh),
                    (&mut *gate as *mut RefreshGate).cast(),
                );
                // Input may be due before the resumed refresh timer. Commit
                // final geometry before it can hit-test aligned new widgets.
                s::lv_obj_update_layout(s::lv_screen_active());
                // A paused refresh timer is also LVGL's normal idle state.
                // Invalidation requests publication through its default handler.
                s::lv_obj_invalidate(s::lv_screen_active());
                s::lv_timer_resume(gate.timer);
                s::lv_timer_ready(gate.timer);
            }
        }
    }
}
