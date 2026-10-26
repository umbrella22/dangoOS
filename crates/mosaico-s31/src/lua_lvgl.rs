//! Foreground Lua LVGL service. Call only on the existing native UI owner.
//! Lua owns callback closures; this service owns native widgets and pixel copies.
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    ffi::CString,
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    cell::{Cell, RefCell},
    ptr,
};
use lvgl::sys as s;
use mosaico_lua::ui::{UiCommand, UiReply};
use serde_json::{Value, json};
unsafe extern "C" {
    fn lv_image_cache_drop(src: *const core::ffi::c_void);
}
const OBJECT_MAX: usize = 512;
const PIXELS_MAX: usize = 2 * 1024 * 1024;
struct Event {
    callback: u64,
    name: String,
    queue: Rc<RefCell<VecDeque<u64>>>,
}
struct Record {
    ptr: *mut s::lv_obj_t,
    alive: Cell<bool>,
    kind: String,
    pixels: Vec<u16>,
    src: Option<CString>,
    image_bytes: usize,
    events: Vec<Box<Event>>,
    label: *mut s::lv_obj_t,
    raw: Option<Box<s::lv_image_dsc_t>>,
}
unsafe extern "C" fn deleted(e: *mut s::lv_event_t) {
    unsafe {
        let r = &*(s::lv_event_get_user_data(e) as *const Record);
        r.alive.set(false);
    }
}
unsafe extern "C" fn event(e: *mut s::lv_event_t) {
    unsafe {
        let event = &*(s::lv_event_get_user_data(e) as *const Event);
        let mut q = event.queue.borrow_mut();
        if q.len() >= 256 {
            q.pop_front();
        }
        q.push_back(event.callback);
    }
}
/// Tracks successful foreground UI requests so a delay-only script publishes
/// its rebuilt tree before sleeping. Shared by the device runner and host probe.
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
// LVGL resumes its refresh timer on every invalidation. Run after the
// display's default REFR_REQUEST handler to retain the completed framebuffer.
struct RefreshGate {
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
/// The native UI owner must release/drop this service before deleting its display.
pub struct LuaLvglService {
    width: i32,
    height: i32,
    active: bool,
    previous: *mut s::lv_obj_t,
    next: u64,
    objects: BTreeMap<u64, Box<Record>>,
    queue: Rc<RefCell<VecDeque<u64>>>,
    root: String,
    retired: Vec<u64>,
    driver: Option<Box<DriverState>>,
    // Hold refresh and this display's pointer read timers while its tree is
    // incomplete. System raw input and other timers continue to run.
    redraw: Option<Box<RefreshGate>>,
    // #region debug-point popup-shadow-trigger
    #[cfg(feature = "native-ui-diagnostics")]
    popup_diagnostics: crate::telemetry::popup_shadow::Diagnostics,
    // #endregion debug-point popup-shadow-trigger
}
impl LuaLvglService {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width: width.into(),
            height: height.into(),
            active: false,
            previous: ptr::null_mut(),
            next: 1,
            objects: BTreeMap::new(),
            queue: Rc::new(RefCell::new(VecDeque::new())),
            root: String::new(),
            retired: Vec::new(),
            driver: None,
            redraw: None,
            #[cfg(feature = "native-ui-diagnostics")]
            popup_diagnostics: crate::telemetry::popup_shadow::Diagnostics::new(),
        }
    }
    // #region debug-point popup-shadow-access
    #[cfg(feature = "native-ui-diagnostics")]
    pub fn popup_diagnostics(&mut self) -> &mut crate::telemetry::popup_shadow::Diagnostics {
        &mut self.popup_diagnostics
    }
    // #endregion debug-point popup-shadow-access
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn object_count(&self) -> usize {
        self.objects.values().filter(|r| r.alive.get()).count()
    }
    pub fn pixel_bytes(&self) -> usize {
        self.objects
            .values()
            .map(|r| r.pixels.len() * 2 + r.image_bytes)
            .sum()
    }
    pub fn execute(&mut self, command: UiCommand) -> Result<UiReply, String> {
        let args: Value = serde_json::from_str(&command.args)
            .map_err(|_| String::from("invalid UI command JSON"))?;
        self.root = command.root;
        let result = unsafe { self.dispatch(&command.op, &args, command.bytes) };
        for r in self.objects.values().filter(|r| !r.alive.get()) {
            self.retired.extend(r.events.iter().map(|e| e.callback));
            if let Some(src) = &r.src {
                unsafe {
                    lv_image_cache_drop(src.as_ptr().cast());
                }
            }
            if let Some(raw) = &r.raw {
                unsafe {
                    lv_image_cache_drop((&**raw as *const s::lv_image_dsc_t).cast());
                }
            }
        }
        self.objects.retain(|_, r| r.alive.get());
        result.map(|v| {
            UiReply::json(
                json!({"values":v,"retired":core::mem::take(&mut self.retired)}).to_string(),
            )
        })
    }
    pub fn release(&mut self) {
        // #region debug-point popup-shadow-release
        #[cfg(feature = "native-ui-diagnostics")]
        self.popup_diagnostics.cancel();
        // #endregion debug-point popup-shadow-release
        if !self.active {
            self.finish_redraw();
            return;
        }
        unsafe {
            if !self.previous.is_null() {
                s::lv_screen_load(self.previous);
            }
            let screens: Vec<_> = self
                .objects
                .values()
                .filter(|r| r.alive.get() && r.kind == "screen")
                .map(|r| r.ptr)
                .collect();
            for p in screens {
                s::lv_obj_delete(p);
            }
            for r in self.objects.values() {
                self.retired.extend(r.events.iter().map(|e| e.callback));
                if let Some(src) = &r.src {
                    lv_image_cache_drop(src.as_ptr().cast());
                }
                if let Some(raw) = &r.raw {
                    lv_image_cache_drop((&**raw as *const s::lv_image_dsc_t).cast());
                }
            }
            self.objects.clear();
            self.queue.borrow_mut().clear();
            self.active = false;
            if let Some(driver) = self.driver.take() {
                s::lv_fs_remove_drive(b'D' as _);
                drop(driver);
            }
        }
        // Keep pointer input held until the old screen is active and all
        // owned targets have been deleted, including on error/stop teardown.
        self.finish_redraw();
    }
    fn begin_redraw(&mut self) {
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
    fn finish_redraw(&mut self) {
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
    unsafe fn add(&mut self, p: *mut s::lv_obj_t, kind: &str) -> Result<u64, String> {
        unsafe {
            if p.is_null() {
                return Err("LVGL allocation failed".into());
            }
            if self.objects.len() >= OBJECT_MAX {
                s::lv_obj_delete(p);
                return Err("LVGL object limit exceeded".into());
            }
            let mut r = Box::new(Record {
                ptr: p,
                alive: Cell::new(true),
                kind: kind.into(),
                pixels: Vec::new(),
                src: None,
                image_bytes: 0,
                events: Vec::new(),
                label: ptr::null_mut(),
                raw: None,
            });
            s::lv_obj_add_event_cb(
                p,
                Some(deleted),
                s::lv_event_code_t_LV_EVENT_DELETE,
                (&mut *r as *mut Record).cast(),
            );
            let id = self.next;
            self.next = self
                .next
                .checked_add(1)
                .ok_or("LVGL handle space exhausted")?;
            self.objects.insert(id, r);
            Ok(id)
        }
    }
    fn get(&self, id: u64) -> Result<&Record, String> {
        self.objects
            .get(&id)
            .filter(|r| r.alive.get())
            .map(Box::as_ref)
            .ok_or_else(|| "deleted or invalid LVGL object".into())
    }
    unsafe fn dispatch(&mut self, op: &str, a: &Value, bytes: Vec<u8>) -> Result<Value, String> {
        unsafe {
            if op == "init" {
                if self.active {
                    return Err("LVGL already initialized".into());
                }
                self.install_driver()?;
                self.previous = s::lv_screen_active();
                self.active = true;
                return Ok(json!([true]));
            }
            if op == "deinit" {
                self.release();
                return Ok(json!([true]));
            }
            if op == "frame_end" {
                self.finish_redraw();
                // #region debug-point popup-shadow-publication
                #[cfg(feature = "native-ui-diagnostics")]
                self.popup_diagnostics.frame_end();
                // #endregion debug-point popup-shadow-publication
                return Ok(json!([true]));
            }
            if !self.active {
                return Err("LVGL not initialized".into());
            }
            if op == "events" {
                return Ok(Value::Array(
                    self.queue
                        .borrow_mut()
                        .drain(..)
                        .map(|v| json!(v))
                        .collect(),
                ));
            }
            if op == "screen" {
                let p = s::lv_screen_active();
                let id = self
                    .objects
                    .iter()
                    .find(|(_, r)| r.ptr == p && r.alive.get())
                    .map(|(id, _)| *id)
                    .ok_or("active screen is not owned by this VM")?;
                return Ok(json!([id]));
            }
            if op == "create" {
                let kind = text(&a[0])?;
                let parent = a[1].as_u64().unwrap_or(0);
                let parent = if parent == 0 {
                    ptr::null_mut()
                } else {
                    self.get(parent)?.ptr
                };
                let opts = &a[2];
                if kind == "screen" && !parent.is_null() {
                    return Err("screen must not have a parent".into());
                }
                if kind != "screen" && parent.is_null() {
                    return Err("widget needs an owned parent".into());
                }
                let p = match kind {
                    "screen" | "object" | "container" => s::lv_obj_create(parent),
                    "label" => s::lv_label_create(parent),
                    "button" => s::lv_button_create(parent),
                    "canvas" => s::lv_canvas_create(parent),
                    "image" => s::lv_image_create(parent),
                    "msgbox" => s::lv_msgbox_create(parent),
                    _ => return Err(format!("unsupported LVGL widget: {kind}")),
                };
                let id = self.add(p, kind)?;
                if kind == "screen" {
                    s::lv_obj_set_size(p, self.width, self.height);
                }
                if let Err(e) = self.options(id, opts) {
                    s::lv_obj_delete(p);
                    return Err(e);
                }
                if kind == "canvas" {
                    if let Err(e) = self.canvas(id, None) {
                        s::lv_obj_delete(p);
                        return Err(e);
                    }
                }
                // #region debug-point popup-shadow-created
                #[cfg(feature = "native-ui-diagnostics")]
                if kind == "msgbox" {
                    self.popup_diagnostics
                        .action(crate::telemetry::popup_shadow::Cause::Open);
                }
                // #endregion debug-point popup-shadow-created
                return Ok(json!([id]));
            }
            let id = uint(&a[0])?;
            if op == "is_valid" {
                return Ok(json!([self.get(id).is_ok()]));
            }
            let p = self.get(id)?.ptr;
            match op {
                "get_size" => {
                    s::lv_obj_update_layout(p);
                    Ok(json!([s::lv_obj_get_width(p), s::lv_obj_get_height(p)]))
                }
                "get_pos" => {
                    s::lv_obj_update_layout(p);
                    Ok(json!([s::lv_obj_get_x(p), s::lv_obj_get_y(p)]))
                }
                "set_size" => {
                    let (w, h) = (coord(&a[1])?, coord(&a[2])?);
                    if w < 1 || h < 1 {
                        return Err("positive widget size required".into());
                    }
                    s::lv_obj_set_size(p, w, h);
                    Ok(json!([true]))
                }
                "set_pos" => {
                    s::lv_obj_set_pos(p, coord(&a[1])?, coord(&a[2])?);
                    Ok(json!([true]))
                }
                "align" => {
                    s::lv_obj_align(
                        p,
                        align(text(&a[1])?)?,
                        optional_coord(&a[2], 0)?,
                        optional_coord(&a[3], 0)?,
                    );
                    Ok(json!([true]))
                }
                "set_style" => {
                    self.style(p, &a[1], false)?;
                    Ok(json!([true]))
                }
                "set_text" => {
                    self.set_text(id, text(&a[1])?)?;
                    Ok(json!([true]))
                }
                "set_flex" => {
                    let opts = &a[1];
                    let flow = match opts["flow"].as_str().unwrap_or("row") {
                        "row" => s::lv_flex_flow_t_LV_FLEX_FLOW_ROW,
                        "column" => s::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN,
                        "row_wrap" => s::lv_flex_flow_t_LV_FLEX_FLOW_ROW_WRAP,
                        "column_wrap" => s::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN_WRAP,
                        _ => return Err("unsupported flex flow".into()),
                    };
                    s::lv_obj_set_flex_flow(p, flow);
                    s::lv_obj_set_flex_align(
                        p,
                        flex(opts["main"].as_str().unwrap_or("start"))?,
                        flex(opts["cross"].as_str().unwrap_or("start"))?,
                        flex(opts["track"].as_str().unwrap_or("start"))?,
                    );
                    Ok(json!([true]))
                }
                "set_scroll" => {
                    let o = &a[1];
                    let dir = match o["dir"].as_str().unwrap_or("all") {
                        "none" => 0,
                        "hor" => 3,
                        "ver" => 12,
                        "all" => 15,
                        _ => return Err("unsupported scroll direction".into()),
                    };
                    s::lv_obj_set_scroll_dir(p, dir);
                    let bar = match o["scrollbar"].as_str().unwrap_or("auto") {
                        "off" => 0,
                        "on" => 1,
                        "active" => 2,
                        "auto" => 3,
                        _ => return Err("unsupported scrollbar mode".into()),
                    };
                    s::lv_obj_set_scrollbar_mode(p, bar);
                    Ok(json!([true]))
                }
                "load" => {
                    if self.get(id)?.kind != "screen" {
                        return Err("load requires screen".into());
                    }
                    s::lv_screen_load(p);
                    Ok(json!([true]))
                }
                "clean" => {
                    if p == s::lv_screen_active() {
                        self.begin_redraw();
                    }
                    s::lv_obj_clean(p);
                    self.objects.get_mut(&id).unwrap().label = ptr::null_mut();
                    Ok(json!([true]))
                }
                "delete" | "close" | "close_async" => {
                    if p == s::lv_screen_active() {
                        return Err("cannot delete active screen; deinit first".into());
                    }
                    // #region debug-point popup-shadow-closed
                    #[cfg(feature = "native-ui-diagnostics")]
                    let popup = self.get(id)?.kind == "msgbox";
                    s::lv_obj_delete(p);
                    #[cfg(feature = "native-ui-diagnostics")]
                    if popup {
                        self.popup_diagnostics.action(if op == "delete" {
                            crate::telemetry::popup_shadow::Cause::Delete
                        } else {
                            crate::telemetry::popup_shadow::Cause::Close
                        });
                    }
                    // #endregion debug-point popup-shadow-closed
                    Ok(json!([true]))
                }
                "on" => {
                    let code = event_code(text(&a[1])?)?;
                    let callback = uint(&a[2])?;
                    let r = self.objects.get_mut(&id).unwrap();
                    if r.events.len() >= 32 {
                        return Err("too many object callbacks".into());
                    }
                    let mut e = Box::new(Event {
                        callback,
                        name: text(&a[1])?.into(),
                        queue: Rc::clone(&self.queue),
                    });
                    s::lv_obj_add_event_cb(p, Some(event), code, (&mut *e as *mut Event).cast());
                    r.events.push(e);
                    Ok(json!([callback]))
                }
                "off" => {
                    let r = self.objects.get_mut(&id).unwrap();
                    let remove: Vec<_> = r
                        .events
                        .iter()
                        .filter(|e| {
                            a[1].is_null()
                                || a[1].as_u64() == Some(e.callback)
                                || a[1].as_str() == Some(e.name.as_str())
                        })
                        .map(|e| (&**e as *const Event).cast_mut())
                        .collect();
                    for e in &remove {
                        s::lv_obj_remove_event_cb_with_user_data(p, Some(event), (*e).cast());
                    }
                    r.events
                        .retain(|e| !remove.contains(&(&**e as *const Event).cast_mut()));
                    Ok(json!([remove.len()]))
                }
                "add_footer_button" => {
                    if self.get(id)?.kind != "msgbox" {
                        return Err("footer requires msgbox".into());
                    }
                    let text = cstring(text(&a[1])?)?;
                    let child = s::lv_msgbox_add_footer_button(p, text.as_ptr());
                    let id = self.add(child, "button")?;
                    Ok(json!([id]))
                }
                "set_rgb565_data" => {
                    let order = a[1].as_str().unwrap_or("le");
                    if order != "le" && order != "be" {
                        return Err("RGB565 byte order must be le or be".into());
                    }
                    self.canvas(id, Some((bytes, order == "be")))?;
                    Ok(json!([true]))
                }
                "set_pivot" | "set_rotation" => {
                    if self.get(id)?.kind != "image" {
                        return Err("image required".into());
                    }
                    if op == "set_pivot" {
                        let x = coord(&a[1])?;
                        let y = coord(&a[2])?;
                        if i16::try_from(x).is_err() || i16::try_from(y).is_err() {
                            return Err("pivot outside signed 16-bit coordinates".into());
                        }
                        s::lv_image_set_pivot(p, x, y);
                    } else {
                        s::lv_image_set_rotation(p, coord(&a[1])?.rem_euclid(3600));
                    }
                    Ok(json!([true]))
                }
                "set_raw_rgb565" => {
                    self.raw_image(
                        id,
                        coord(&a[1])?,
                        coord(&a[2])?,
                        bytes,
                        a[3]["swapped"].as_bool().unwrap_or(false),
                    )?;
                    Ok(json!([true]))
                }
                "fill_bg" | "set_px" | "get_px" => {
                    let r = self.get(id)?;
                    if r.kind != "canvas" {
                        return Err("canvas required".into());
                    }
                    let w = s::lv_obj_get_width(p) as usize;
                    let h = s::lv_obj_get_height(p) as usize;
                    if w.checked_mul(h) != Some(r.pixels.len()) {
                        return Err("canvas resized; upload pixels first".into());
                    }
                    let at = if op != "fill_bg" {
                        let x = coord(&a[1])?;
                        let y = coord(&a[2])?;
                        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
                            return Err("pixel outside canvas".into());
                        }
                        y as usize * w + x as usize
                    } else {
                        0
                    };
                    if op == "get_px" {
                        let v = r.pixels[at];
                        return Ok(
                            json!([{"r":((v>>11)&31) as u32*255/31,"g":((v>>5)&63) as u32*255/63,"b":(v&31) as u32*255/31,"a":255}]),
                        );
                    }
                    let ci = if op == "fill_bg" { 1 } else { 3 };
                    let rgb = color(&a[ci])?;
                    let opa = if a[ci + 1].is_null() {
                        255
                    } else {
                        opacity(&a[ci + 1])?
                    } as u32;
                    let r = self.objects.get_mut(&id).unwrap();
                    let pixels = if op == "fill_bg" {
                        &mut r.pixels[..]
                    } else {
                        &mut r.pixels[at..at + 1]
                    };
                    for old in pixels {
                        let red = (((rgb >> 16) & 255) * opa
                            + (((*old >> 11) & 31) as u32 * 255 / 31) * (255 - opa)
                            + 127)
                            / 255;
                        let green = (((rgb >> 8) & 255) * opa
                            + (((*old >> 5) & 63) as u32 * 255 / 63) * (255 - opa)
                            + 127)
                            / 255;
                        let blue = ((rgb & 255) * opa
                            + ((*old & 31) as u32 * 255 / 31) * (255 - opa)
                            + 127)
                            / 255;
                        *old = ((red >> 3) << 11 | (green >> 2) << 5 | blue >> 3) as u16;
                    }
                    s::lv_obj_invalidate(p);
                    Ok(json!([true]))
                }
                "add_flag" | "clear_flag" => {
                    let flag = match text(&a[1])? {
                        "CLICKABLE" => s::lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE,
                        "PRESS_LOCK" => s::lv_obj_flag_t_LV_OBJ_FLAG_PRESS_LOCK,
                        "SCROLLABLE" => s::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE,
                        "HIDDEN" => s::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN,
                        _ => return Err("unsupported object flag".into()),
                    };
                    if op == "add_flag" {
                        s::lv_obj_add_flag(p, flag)
                    } else {
                        s::lv_obj_remove_flag(p, flag)
                    };
                    Ok(json!([true]))
                }
                "move_foreground" => {
                    s::lv_obj_move_to_index(p, -1);
                    Ok(json!([true]))
                }
                _ => Err(format!("unsupported LVGL operation: {op}")),
            }
        }
    }
    unsafe fn set_text(&mut self, id: u64, text: &str) -> Result<(), String> {
        unsafe {
            let text = cstring(text)?;
            let r = self
                .objects
                .get_mut(&id)
                .filter(|r| r.alive.get())
                .ok_or("deleted or invalid LVGL object")?;
            if r.kind == "label" {
                s::lv_label_set_text(r.ptr, text.as_ptr());
            } else if r.kind == "button" {
                if r.label.is_null() {
                    r.label = s::lv_label_create(r.ptr);
                    if r.label.is_null() {
                        return Err("label allocation failed".into());
                    }
                    s::lv_obj_center(r.label);
                }
                s::lv_label_set_text(r.label, text.as_ptr());
            } else {
                return Err("object does not have text".into());
            }
            Ok(())
        }
    }
    unsafe fn options(&mut self, id: u64, o: &Value) -> Result<(), String> {
        unsafe {
            let p = self.get(id)?.ptr;
            self.style(p, o, true)?;
            if self.get(id)?.kind != "msgbox" {
                if let Some(t) = o.get("text") {
                    self.set_text(id, text(t)?)?;
                }
            }
            if let Some(w) = o.get("w") {
                s::lv_obj_set_width(p, coord(w)?);
            }
            if let Some(h) = o.get("h") {
                s::lv_obj_set_height(p, coord(h)?);
            }
            let x = optional_coord(&o["x"], 0)?;
            let y = optional_coord(&o["y"], 0)?;
            if let Some(a) = o["align"].as_str() {
                s::lv_obj_align(p, align(a)?, x, y);
            } else {
                s::lv_obj_set_pos(p, x, y);
            }
            if let Some(src) = o["src"].as_str() {
                if self.get(id)?.kind != "image" {
                    return Err("src requires image".into());
                }
                if !src.starts_with("D:/") {
                    return Err("file images require scoped D:/ path".into());
                }
                self.file_image(id, src)?;
            }
            if self.get(id)?.kind == "msgbox" {
                if let Some(t) = o["title"].as_str() {
                    let t = cstring(t)?;
                    s::lv_msgbox_add_title(p, t.as_ptr());
                }
                if let Some(t) = o["text"].as_str() {
                    let t = cstring(t)?;
                    s::lv_msgbox_add_text(p, t.as_ptr());
                }
            }
            Ok(())
        }
    }
    unsafe fn style(&self, p: *mut s::lv_obj_t, o: &Value, options: bool) -> Result<(), String> {
        unsafe {
            if !o.is_object() && !o.as_array().is_some_and(Vec::is_empty) {
                return Err("style/options table required".into());
            }
            if let Some(map) = o.as_object() {
                for (k, v) in map {
                    match k.as_str() {
                        "bg_color" => {
                            s::lv_obj_set_style_bg_color(p, s::lv_color_hex(color(v)?), 0)
                        }
                        "text_color" => {
                            s::lv_obj_set_style_text_color(p, s::lv_color_hex(color(v)?), 0)
                        }
                        "border_color" => {
                            s::lv_obj_set_style_border_color(p, s::lv_color_hex(color(v)?), 0)
                        }
                        "bg_opa" => s::lv_obj_set_style_bg_opa(p, opacity(v)?, 0),
                        "opa" => s::lv_obj_set_style_opa(p, opacity(v)?, 0),
                        "radius" => s::lv_obj_set_style_radius(p, coord(v)?, 0),
                        "border_width" => s::lv_obj_set_style_border_width(p, coord(v)?, 0),
                        "pad" => s::lv_obj_set_style_pad_all(p, coord(v)?, 0),
                        "pad_row" => s::lv_obj_set_style_pad_row(p, coord(v)?, 0),
                        "pad_column" => s::lv_obj_set_style_pad_column(p, coord(v)?, 0),
                        "color_format" if options => {
                            if v.as_str() != Some("rgb565") {
                                return Err("only RGB565 canvas format supported".into());
                            }
                        }
                        "w" | "h" | "x" | "y" | "align" | "text" | "src" | "title" if options => {}
                        _ => return Err(format!("unsupported LVGL style/option: {k}")),
                    }
                }
            }
            Ok(())
        }
    }
    unsafe fn canvas(&mut self, id: u64, input: Option<(Vec<u8>, bool)>) -> Result<(), String> {
        unsafe {
            let r = self.get(id)?;
            if r.kind != "canvas" {
                return Err("canvas required".into());
            }
            s::lv_obj_update_layout(r.ptr);
            let w = s::lv_obj_get_width(r.ptr);
            let h = s::lv_obj_get_height(r.ptr);
            if w < 1 || h < 1 {
                return Err("invalid canvas dimensions".into());
            }
            let n = (w as usize)
                .checked_mul(h as usize)
                .ok_or("canvas overflow")?;
            let previous = r.pixels.len() * 2;
            let current = self.pixel_bytes();
            if n > PIXELS_MAX / 2 || current - previous + n * 2 > PIXELS_MAX {
                return Err("LVGL canvas pixel budget exceeded".into());
            }
            let pixels = if let Some((bytes, be)) = input {
                if bytes.len() != n * 2 {
                    return Err("RGB565 upload size mismatch".into());
                }
                bytes
                    .chunks_exact(2)
                    .map(|p| {
                        if be {
                            u16::from_be_bytes([p[0], p[1]])
                        } else {
                            u16::from_le_bytes([p[0], p[1]])
                        }
                    })
                    .collect()
            } else {
                alloc::vec![0;n]
            };
            let r = self.objects.get_mut(&id).unwrap();
            r.pixels = pixels;
            s::lv_canvas_set_buffer(
                r.ptr,
                r.pixels.as_mut_ptr().cast(),
                w,
                h,
                s::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
            );
            s::lv_obj_invalidate(r.ptr);
            Ok(())
        }
    }
    unsafe fn raw_image(
        &mut self,
        id: u64,
        w: i32,
        h: i32,
        bytes: Vec<u8>,
        be: bool,
    ) -> Result<(), String> {
        unsafe {
            let r = self.get(id)?;
            if r.kind != "image" {
                return Err("image required".into());
            }
            if w < 1 || w > 32767 || h < 1 || h > 65535 {
                return Err("invalid raw image dimensions".into());
            }
            let size = (w as usize)
                .checked_mul(h as usize)
                .and_then(|v| v.checked_mul(2))
                .ok_or("raw image overflow")?;
            if bytes.len() != size {
                return Err("RGB565 upload size mismatch".into());
            }
            if size > PIXELS_MAX
                || self.pixel_bytes() - r.pixels.len() * 2 - r.image_bytes + size > PIXELS_MAX
            {
                return Err("LVGL pixel budget exceeded".into());
            }
            let pixels: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| {
                    if be {
                        u16::from_be_bytes([p[0], p[1]])
                    } else {
                        u16::from_le_bytes([p[0], p[1]])
                    }
                })
                .collect();
            let mut descriptor = Box::new(s::lv_image_dsc_t::default());
            descriptor.header.set_magic(s::LV_IMAGE_HEADER_MAGIC);
            descriptor
                .header
                .set_cf(s::lv_color_format_t_LV_COLOR_FORMAT_RGB565);
            descriptor.header.set_w(w as u32);
            descriptor.header.set_h(h as u32);
            descriptor.header.set_stride(w as u32 * 2);
            descriptor.data_size = size as u32;
            descriptor.data = pixels.as_ptr().cast();
            let r = self.objects.get_mut(&id).unwrap();
            if let Some(src) = &r.src {
                lv_image_cache_drop(src.as_ptr().cast());
            }
            if let Some(raw) = &r.raw {
                lv_image_cache_drop((&**raw as *const s::lv_image_dsc_t).cast());
            }
            s::lv_image_set_src(r.ptr, (&*descriptor as *const s::lv_image_dsc_t).cast());
            r.pixels = pixels;
            r.raw = Some(descriptor);
            r.src = None;
            r.image_bytes = 0;
            Ok(())
        }
    }
    unsafe fn install_driver(&mut self) -> Result<(), String> {
        unsafe {
            if !s::lv_fs_get_drv(b'D' as _).is_null() {
                return Err("LVGL D: drive already owned".into());
            }
            let mut state = Box::new(DriverState {
                driver: s::lv_fs_drv_t::default(),
                root: self.root.clone(),
            });
            s::lv_fs_drv_init(&mut state.driver);
            state.driver.letter = b'D' as _;
            state.driver.user_data = (&mut *state as *mut DriverState).cast();
            state.driver.open_cb = Some(fs_open);
            state.driver.close_cb = Some(fs_close);
            state.driver.read_cb = Some(fs_read);
            state.driver.seek_cb = Some(fs_seek);
            state.driver.tell_cb = Some(fs_tell);
            s::lv_fs_drv_register(&mut state.driver);
            self.driver = Some(state);
            Ok(())
        }
    }
    unsafe fn file_image(&mut self, id: u64, src: &str) -> Result<(), String> {
        unsafe {
            let relative = src.strip_prefix("D:/").ok_or("D:/ image path required")?;
            if relative.is_empty()
                || relative
                    .split('/')
                    .any(|v| v.is_empty() || v == "." || v == "..")
                || relative.contains('\\')
            {
                return Err("unsafe image path".into());
            }
            let path = cstring(src)?;
            let mut header = s::lv_image_header_t::default();
            if s::lv_image_decoder_get_info(path.as_ptr().cast(), &mut header)
                != s::lv_result_t_LV_RESULT_OK
            {
                return Err("LVGL image decoder rejected file".into());
            }
            let image_bytes = header.w() as usize * header.h() as usize * 4;
            if header.w() == 0
                || header.h() == 0
                || image_bytes > PIXELS_MAX
                || self.pixel_bytes() - self.get(id)?.image_bytes + image_bytes > PIXELS_MAX
            {
                return Err("file image exceeds pixel budget".into());
            }
            let r = self.objects.get_mut(&id).unwrap();
            if let Some(old) = &r.src {
                lv_image_cache_drop(old.as_ptr().cast());
            }
            r.src = Some(path);
            r.image_bytes = image_bytes;
            s::lv_image_set_src(r.ptr, r.src.as_ref().unwrap().as_ptr().cast());
            Ok(())
        }
    }
}
impl Drop for LuaLvglService {
    fn drop(&mut self) {
        self.release();
    }
}
fn text(v: &Value) -> Result<&str, String> {
    v.as_str().ok_or_else(|| "string required".into())
}
fn uint(v: &Value) -> Result<u64, String> {
    v.as_u64()
        .ok_or_else(|| "positive object identifier required".into())
}
fn coord(v: &Value) -> Result<i32, String> {
    let n = v.as_i64().ok_or("integer coordinate required")?;
    i32::try_from(n).map_err(|_| "coordinate out of range".into())
}
fn optional_coord(v: &Value, default: i32) -> Result<i32, String> {
    if v.is_null() { Ok(default) } else { coord(v) }
}
fn cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "text contains NUL".into())
}
fn color(v: &Value) -> Result<u32, String> {
    let n = if let Some(v) = v.as_u64() {
        v
    } else {
        u64::from_str_radix(
            text(v)?
                .strip_prefix('#')
                .ok_or("hex color must start with #")?,
            16,
        )
        .map_err(|_| "invalid color")?
    };
    if n > 0xffffff {
        Err("color outside RGB888".into())
    } else {
        Ok(n as u32)
    }
}
fn opacity(v: &Value) -> Result<u8, String> {
    u8::try_from(v.as_u64().ok_or("integer opacity required")?)
        .map_err(|_| "opacity outside 0..255".into())
}
fn align(v: &str) -> Result<s::lv_align_t, String> {
    Ok(match v {
        "default" | "top_left" => s::lv_align_t_LV_ALIGN_TOP_LEFT,
        "top_mid" | "top" => s::lv_align_t_LV_ALIGN_TOP_MID,
        "top_right" => s::lv_align_t_LV_ALIGN_TOP_RIGHT,
        "bottom_left" => s::lv_align_t_LV_ALIGN_BOTTOM_LEFT,
        "bottom_mid" | "bottom" => s::lv_align_t_LV_ALIGN_BOTTOM_MID,
        "bottom_right" => s::lv_align_t_LV_ALIGN_BOTTOM_RIGHT,
        "left_mid" | "left" => s::lv_align_t_LV_ALIGN_LEFT_MID,
        "right_mid" | "right" => s::lv_align_t_LV_ALIGN_RIGHT_MID,
        "center" | "centre" => s::lv_align_t_LV_ALIGN_CENTER,
        _ => return Err("unsupported alignment".into()),
    })
}
fn flex(v: &str) -> Result<s::lv_flex_align_t, String> {
    Ok(match v {
        "start" => s::lv_flex_align_t_LV_FLEX_ALIGN_START,
        "end" => s::lv_flex_align_t_LV_FLEX_ALIGN_END,
        "center" => s::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
        "space_between" => s::lv_flex_align_t_LV_FLEX_ALIGN_SPACE_BETWEEN,
        "space_around" => s::lv_flex_align_t_LV_FLEX_ALIGN_SPACE_AROUND,
        "space_evenly" => s::lv_flex_align_t_LV_FLEX_ALIGN_SPACE_EVENLY,
        _ => return Err("unsupported flex alignment".into()),
    })
}

#[cfg(all(test, feature = "native-ui-diagnostics"))]
mod popup_diagnostic_tests {
    use super::*;
    use crate::telemetry::popup_shadow::{Cause, Line, SETTLE_US};

    fn call(service: &mut LuaLvglService, op: &str, args: Value) -> Result<Value, String> {
        let reply = service.execute(UiCommand {
            op: op.into(),
            args: args.to_string(),
            bytes: Vec::new(),
            root: "/tmp".into(),
        })?;
        Ok(serde_json::from_str::<Value>(&reply.json).unwrap()["values"].clone())
    }
    unsafe extern "C" fn flush(display: *mut s::lv_display_t, _: *const s::lv_area_t, _: *mut u8) {
        unsafe { s::lv_display_flush_ready(display) };
    }
    fn sample(service: &mut LuaLvglService, pixels: &[u16], at: u64, seq: u32, cause: Cause) {
        assert!(
            !service
                .popup_diagnostics()
                .sample_if_ready(at, true, Some(pixels))
        );
        call(service, "frame_end", json!([])).unwrap();
        unsafe { s::lv_timer_handler() };
        service.popup_diagnostics().handler_finished();
        assert!(
            service
                .popup_diagnostics()
                .sample_if_ready(at + SETTLE_US, true, Some(pixels))
        );
        assert!(
            matches!(service.popup_diagnostics().next_line(at + SETTLE_US),
            Some(Line::Begin { seq: q, cause: c, .. }) if q == seq && c == cause)
        );
        for index in 1..=121 {
            assert!(
                service
                    .popup_diagnostics()
                    .next_line(at + SETTLE_US + index * 20_000)
                    .is_some()
            );
        }
        assert!(
            service
                .popup_diagnostics()
                .next_line(at + SETTLE_US + 122 * 20_000)
                .is_none()
        );
    }
    #[test]
    fn real_service_only_samples_successful_msgbox_actions_after_frame_end() {
        let _lock = crate::test_support::lock();
        unsafe { lvgl::deinit() };
        lvgl::init();
        let mut buffer = alloc::vec![0u16; 480 * 48];
        let display = unsafe {
            let display = s::lv_display_create(480, 480);
            s::lv_display_set_color_format(display, s::lv_color_format_t_LV_COLOR_FORMAT_RGB565);
            s::lv_display_set_buffers(
                display,
                buffer.as_mut_ptr().cast(),
                ptr::null_mut(),
                (buffer.len() * 2) as u32,
                s::lv_display_render_mode_t_LV_DISPLAY_RENDER_MODE_PARTIAL,
            );
            s::lv_display_set_flush_cb(display, Some(flush));
            display
        };
        let mut service = LuaLvglService::new(480, 480);
        call(&mut service, "init", json!([])).unwrap();
        let screen = call(&mut service, "create", json!(["screen", 0, {}])).unwrap()[0]
            .as_u64()
            .unwrap();
        call(&mut service, "load", json!([screen])).unwrap();
        let label = call(
            &mut service,
            "create",
            json!(["label", screen, {"text":"plain"}]),
        )
        .unwrap()[0]
            .as_u64()
            .unwrap();
        call(&mut service, "delete", json!([label])).unwrap();
        assert!(
            call(
                &mut service,
                "create",
                json!(["msgbox", screen, {"w":"invalid"}])
            )
            .is_err()
        );
        assert!(call(&mut service, "close", json!([u64::MAX])).is_err());
        let pixels = alloc::vec![0x1234; 480 * 480];
        call(&mut service, "frame_end", json!([])).unwrap();
        service.popup_diagnostics().handler_finished();
        assert!(
            !service
                .popup_diagnostics()
                .sample_if_ready(1_000_000, true, Some(&pixels))
        );
        assert!(service.popup_diagnostics().next_line(1_000_000).is_none());

        let popup = call(
            &mut service,
            "create",
            json!(["msgbox", screen, {"title":"Delete photo?"}]),
        )
        .unwrap()[0]
            .as_u64()
            .unwrap();
        sample(&mut service, &pixels, 2_000_000, 1, Cause::Open);
        call(&mut service, "close", json!([popup])).unwrap();
        assert!(call(&mut service, "close", json!([popup])).is_err());
        sample(&mut service, &pixels, 10_000_000, 2, Cause::Close);
        let popup = call(&mut service, "create", json!(["msgbox", screen, {}])).unwrap()[0]
            .as_u64()
            .unwrap();
        call(&mut service, "delete", json!([popup])).unwrap();
        assert_eq!(
            service.popup_diagnostics().next_line(20_000_000),
            Some(Line::Superseded {
                first: 3,
                last: 3,
                count: 1,
                by: 4
            })
        );
        sample(&mut service, &pixels, 21_000_000, 4, Cause::Delete);
        service.release();
        drop(service);
        unsafe {
            s::lv_display_delete(display);
            lvgl::deinit();
        }
    }
}
fn event_code(v: &str) -> Result<s::lv_event_code_t, String> {
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

struct DriverState {
    driver: s::lv_fs_drv_t,
    root: String,
}
unsafe extern "C" fn fs_open(
    drv: *mut s::lv_fs_drv_t,
    path: *const core::ffi::c_char,
    mode: s::lv_fs_mode_t,
) -> *mut core::ffi::c_void {
    unsafe {
        if mode != s::lv_fs_mode_t_LV_FS_MODE_RD {
            return ptr::null_mut();
        }
        let state = &*((*drv).user_data as *const DriverState);
        let Ok(path) = core::ffi::CStr::from_ptr(path).to_str() else {
            return ptr::null_mut();
        };
        let path = path.trim_start_matches('/');
        if path.is_empty()
            || path.contains('\\')
            || path
                .split('/')
                .any(|v| v.is_empty() || v == "." || v == "..")
        {
            return ptr::null_mut();
        }
        let absolute = format!("{}/{}", state.root, path);
        match mosaico_fs::global::with(|vfs| vfs.open(&absolute, mosaico_fs::OpenFlags::READ)) {
            Ok(fd) => Box::into_raw(Box::new(fd)).cast(),
            Err(_) => ptr::null_mut(),
        }
    }
}
unsafe extern "C" fn fs_close(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
) -> s::lv_fs_res_t {
    unsafe {
        let fd = Box::from_raw(file.cast::<i32>());
        match mosaico_fs::global::with(|vfs| vfs.close(*fd)) {
            Ok(()) => 0,
            Err(_) => 2,
        }
    }
}
unsafe extern "C" fn fs_read(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
    buf: *mut core::ffi::c_void,
    n: u32,
    read: *mut u32,
) -> s::lv_fs_res_t {
    unsafe {
        *read = 0;
        let fd = *file.cast::<i32>();
        let out = core::slice::from_raw_parts_mut(buf.cast::<u8>(), n as usize);
        match mosaico_fs::global::with(|vfs| vfs.read(fd, out)) {
            Ok(n) => {
                *read = n as u32;
                0
            }
            Err(_) => 2,
        }
    }
}
unsafe extern "C" fn fs_seek(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
    pos: u32,
    whence: s::lv_fs_whence_t,
) -> s::lv_fs_res_t {
    unsafe {
        let whence = match whence {
            0 => mosaico_fs::Whence::Set,
            1 => mosaico_fs::Whence::Cur,
            2 => mosaico_fs::Whence::End,
            _ => return 11,
        };
        match mosaico_fs::global::with(|vfs| vfs.seek(*file.cast::<i32>(), pos as i64, whence)) {
            Ok(_) => 0,
            Err(_) => 2,
        }
    }
}
unsafe extern "C" fn fs_tell(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
    pos: *mut u32,
) -> s::lv_fs_res_t {
    unsafe {
        match mosaico_fs::global::with(|vfs| {
            vfs.seek(*file.cast::<i32>(), 0, mosaico_fs::Whence::Cur)
        }) {
            Ok(n) => {
                *pos = n;
                0
            }
            Err(_) => 2,
        }
    }
}
