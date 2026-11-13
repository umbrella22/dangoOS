//! Paired native display events within one UI-thread timer-handler call.

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Stats {
    pub refresh_us: u64,
    pub render_us: u64,
    pub refresh_calls: u32,
    pub render_calls: u32,
    pub without_render: u32,
    pub invalid: u32,
}

#[derive(Clone, Copy)]
enum Event {
    RefreshStart,
    RefreshReady,
    RenderStart,
    RenderReady,
}

#[derive(Clone, Copy, Default)]
struct Recorder {
    stats: Stats,
    active: bool,
    refresh: Option<u64>,
    render: Option<u64>,
    saw_render: bool,
}

impl Recorder {
    fn begin(&mut self) {
        *self = Self {
            active: true,
            ..Self::default()
        };
    }

    fn event(&mut self, event: Event, now: u64) {
        if !self.active {
            return;
        }
        match event {
            Event::RefreshStart => {
                self.stats.invalid += u32::from(self.refresh.is_some() || self.render.is_some());
                self.refresh = Some(now);
                self.saw_render = false;
            }
            Event::RefreshReady => {
                self.stats.invalid += u32::from(self.render.is_some());
                if let Some(us) = self.refresh.take().and_then(|start| now.checked_sub(start)) {
                    self.stats.refresh_us += us;
                    self.stats.refresh_calls += 1;
                    self.stats.without_render += u32::from(!self.saw_render);
                } else {
                    self.stats.invalid += 1;
                }
            }
            Event::RenderStart => {
                self.stats.invalid += u32::from(self.refresh.is_none() || self.render.is_some());
                self.render = Some(now);
                self.saw_render = true;
            }
            Event::RenderReady => {
                self.stats.invalid += u32::from(self.refresh.is_none());
                if let Some(us) = self.render.take().and_then(|start| now.checked_sub(start)) {
                    self.stats.render_us += us;
                    self.stats.render_calls += 1;
                } else {
                    self.stats.invalid += 1;
                }
            }
        }
    }

    fn finish(&mut self) -> Stats {
        self.stats.invalid += u32::from(self.refresh.is_some()) + u32::from(self.render.is_some());
        self.active = false;
        self.stats
    }
}

#[cfg(all(feature = "display-metrics", target_os = "none"))]
mod device {
    use super::{Event, Recorder, Stats};
    use core::cell::Cell;
    use rs_lvgl::sys;

    static RECORDER: critical_section::Mutex<Cell<Recorder>> =
        critical_section::Mutex::new(Cell::new(Recorder {
            stats: Stats {
                refresh_us: 0,
                render_us: 0,
                refresh_calls: 0,
                render_calls: 0,
                without_render: 0,
                invalid: 0,
            },
            active: false,
            refresh: None,
            render: None,
            saw_render: false,
        }));

    fn with<R>(f: impl FnOnce(&mut Recorder) -> R) -> R {
        critical_section::with(|cs| {
            let cell = RECORDER.borrow(cs);
            let mut recorder = cell.get();
            let result = f(&mut recorder);
            cell.set(recorder);
            result
        })
    }

    pub(crate) fn begin() {
        with(Recorder::begin);
    }

    pub(crate) fn finish() -> Stats {
        with(Recorder::finish)
    }

    /// Register only on the single serialized UI display. The callback retains
    /// no display pointer and performs no allocation or logging.
    pub(crate) unsafe fn register(display: *mut sys::lv_display_t) {
        let descriptor = unsafe {
            sys::lv_display_add_event_cb(
                display,
                Some(event),
                sys::lv_event_code_t_LV_EVENT_ALL,
                core::ptr::null_mut(),
            )
        };
        assert!(!descriptor.is_null(), "LVGL refresh metrics registration");
    }

    unsafe extern "C" fn event(event: *mut sys::lv_event_t) {
        // SAFETY: LVGL invokes this callback with its current live event on
        // the UI thread; the callback never stores the native event pointer.
        let kind = match unsafe { sys::lv_event_get_code(event) } {
            sys::lv_event_code_t_LV_EVENT_REFR_START => Event::RefreshStart,
            sys::lv_event_code_t_LV_EVENT_REFR_READY => Event::RefreshReady,
            sys::lv_event_code_t_LV_EVENT_RENDER_START => Event::RenderStart,
            sys::lv_event_code_t_LV_EVENT_RENDER_READY => Event::RenderReady,
            _ => return,
        };
        let now = crate::platform::runtime::time::now_us();
        with(|recorder| recorder.event(kind, now));
    }
}

#[cfg(all(feature = "display-metrics", target_os = "none"))]
pub(crate) use device::{begin, finish, register};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_multiple_refreshes_and_ignores_events_outside_the_handler_window() {
        let mut r = Recorder::default();
        r.event(Event::RefreshStart, 1);
        r.begin();
        for base in [100, 300] {
            r.event(Event::RefreshStart, base);
            r.event(Event::RenderStart, base + 20);
            r.event(Event::RenderReady, base + 80);
            r.event(Event::RefreshReady, base + 100);
        }
        let s = r.finish();
        assert_eq!((s.refresh_us, s.render_us), (200, 120));
        assert_eq!(
            (s.refresh_calls, s.render_calls, s.without_render, s.invalid),
            (2, 2, 0, 0)
        );
        r.event(Event::RenderReady, 999);
        assert_eq!(r.stats.invalid, 0);
    }

    #[test]
    fn counts_refresh_without_render_and_rejects_missing_end_events() {
        let mut r = Recorder::default();
        r.begin();
        r.event(Event::RefreshStart, 10);
        r.event(Event::RefreshReady, 20);
        let s = r.finish();
        assert_eq!((s.refresh_us, s.without_render, s.invalid), (10, 1, 0));
        r.begin();
        r.event(Event::RefreshStart, 30);
        r.event(Event::RenderStart, 40);
        assert_eq!(r.finish().invalid, 2);
    }

    #[test]
    fn malformed_order_and_backwards_timestamps_invalidate_the_sample() {
        let mut r = Recorder::default();
        r.begin();
        r.event(Event::RenderReady, 10);
        assert!(r.finish().invalid > 0);
        r.begin();
        r.event(Event::RefreshStart, 100);
        r.event(Event::RefreshReady, 90);
        let s = r.finish();
        assert_eq!(s.refresh_calls, 0);
        assert!(s.invalid > 0);
    }
}
