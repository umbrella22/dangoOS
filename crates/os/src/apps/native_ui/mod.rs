//! Foreground LVGL service. Call only on the existing native UI owner.
//! Script VMs retain closures; this service retains native widgets and pixel copies.
mod batch;
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
use app_host::ui::{UiCommand, UiReply};
use core::{cell::RefCell, ptr};
use filesystem::DriverState;
use objects::Record;
use refresh::RefreshGate;
pub use refresh::{RedrawBoundary, RpcWindow};
use rs_lvgl::sys as s;
use serde_json::{json, Value};

pub fn limits() -> app_host::NativeUiLimits {
    app_host::NativeUiLimits {
        objects: objects::OBJECT_MAX,
        image_bytes: images::PIXELS_MAX,
    }
}
unsafe extern "C" {
    fn lv_image_cache_drop(src: *const core::ffi::c_void);
}

/// The native UI owner must release/drop this service before deleting its display.
pub struct NativeUiService {
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
    pending: Option<batch::PendingBatch>,
}
impl NativeUiService {
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
            pending: None,
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
    /// Synchronous adapter for host callers. The device UI loop uses submit /
    /// resume so it can process input and lifecycle work between batch slices.
    pub fn execute(&mut self, command: UiCommand) -> Result<UiReply, String> {
        if let Some(reply) = self.submit(command)? {
            return Ok(reply);
        }
        loop {
            if let Some(reply) = self.resume()? {
                return Ok(reply);
            }
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Accept one request on the native UI owner. None retains the request;
    /// resume it on a later owner turn and send only its final reply to the VM.
    pub fn submit(&mut self, command: UiCommand) -> Result<Option<UiReply>, String> {
        if self.has_pending() {
            return Err("UI batch already pending".into());
        }
        if matches!(command.op.as_str(), "batch" | "batch_commit") {
            if command.args.len() > app_host::ui::BATCH_BYTES_MAX {
                return Err("UI batch byte limit exceeded".into());
            }
            if !command.bytes.is_empty() {
                return Err("UI batches do not accept pixel payloads".into());
            }
        }
        let args: Value = serde_json::from_str(&command.args)
            .map_err(|_| String::from("invalid UI command JSON"))?;
        self.root = command.root;
        if matches!(command.op.as_str(), "batch" | "batch_commit") {
            if !self.active {
                return Err("LVGL not initialized".into());
            }
            self.begin_batch(args, command.op == "batch_commit")?;
            return self.resume();
        }
        let result = unsafe { self.dispatch(&command.op, &args, command.bytes) };
        self.finish_reply(result).map(Some)
    }

    /// Execute one continuation slice on the same UI owner as submit.
    pub fn resume(&mut self) -> Result<Option<UiReply>, String> {
        match unsafe { self.batch_slice() } {
            Ok(None) => Ok(None),
            Ok(Some(values)) => self.finish_reply(Ok(values)).map(Some),
            Err(error) => self.finish_reply(Err(error)).map(Some),
        }
    }

    fn finish_reply(&mut self, result: Result<Value, String>) -> Result<UiReply, String> {
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
        if !self.retired.is_empty() {
            self.queue
                .borrow_mut()
                .retain(|id| !self.retired.contains(id));
        }
        result.map(|v| {
            UiReply::json(
                json!({"values":v,"retired":core::mem::take(&mut self.retired)}).to_string(),
            )
        })
    }
    pub fn release(&mut self) {
        // Teardown owns all native objects, including those created by an
        // unfinished batch. No continuation or local handle survives release.
        self.pending = None;
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
impl Drop for NativeUiService {
    fn drop(&mut self) {
        self.release();
    }
}
