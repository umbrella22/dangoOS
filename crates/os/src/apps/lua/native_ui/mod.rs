//! Foreground Lua LVGL service. Call only on the existing native UI owner.
//! Lua owns callback closures; this service owns native widgets and pixel copies.
mod dispatch;
mod events;
mod filesystem;
mod images;
mod objects;

mod refresh;
mod values;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use core::{cell::RefCell, ptr};
use filesystem::DriverState;
use lua_engine::ui::{UiCommand, UiReply};
use objects::Record;
use refresh::RefreshGate;
pub use refresh::{RedrawBoundary, RpcWindow};
use rs_lvgl::sys as s;
use serde_json::{json, Value};
unsafe extern "C" {
    fn lv_image_cache_drop(src: *const core::ffi::c_void);
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
        }
    }

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
}
impl Drop for LuaLvglService {
    fn drop(&mut self) {
        self.release();
    }
}
