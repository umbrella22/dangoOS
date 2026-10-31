//! Stable callback records and the bounded foreground event queue.
use alloc::{collections::VecDeque, rc::Rc, string::String};
use core::cell::RefCell;
use rs_lvgl::sys as s;
pub(super) struct Event {
    pub(super) callback: u64,
    pub(super) name: String,
    pub(super) queue: Rc<RefCell<VecDeque<u64>>>,
}
pub(super) unsafe extern "C" fn event(e: *mut s::lv_event_t) {
    unsafe {
        let event = &*(s::lv_event_get_user_data(e) as *const Event);
        let mut q = event.queue.borrow_mut();
        if q.len() >= 256 {
            q.pop_front();
        }
        q.push_back(event.callback);
    }
}
pub(super) fn event_code(v: &str) -> Result<s::lv_event_code_t, String> {
    Ok(match v {
        "clicked" => s::lv_event_code_t_LV_EVENT_CLICKED,
        "pressed" => s::lv_event_code_t_LV_EVENT_PRESSED,
        "released" => s::lv_event_code_t_LV_EVENT_RELEASED,
        "long_pressed" => s::lv_event_code_t_LV_EVENT_LONG_PRESSED,
        "value_changed" => s::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
        "focused" => s::lv_event_code_t_LV_EVENT_FOCUSED,
        "defocused" => s::lv_event_code_t_LV_EVENT_DEFOCUSED,
        "ready" => s::lv_event_code_t_LV_EVENT_READY,
        "cancel" => s::lv_event_code_t_LV_EVENT_CANCEL,
        _ => return Err("unsupported LVGL event".into()),
    })
}
