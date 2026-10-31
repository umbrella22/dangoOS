//! Native object records, handles, text and style updates.
use super::{events::Event, values::*, LuaLvglService};
use alloc::{boxed::Box, ffi::CString, format, string::String, vec::Vec};
use core::{cell::Cell, ptr};
use rs_lvgl::sys as s;
use serde_json::Value;
const OBJECT_MAX: usize = 512;
pub(super) struct Record {
    pub(super) ptr: *mut s::lv_obj_t,
    pub(super) alive: Cell<bool>,
    pub(super) kind: String,
    pub(super) pixels: Vec<u16>,
    pub(super) src: Option<CString>,
    pub(super) image_bytes: usize,
    pub(super) events: Vec<Box<Event>>,
    pub(super) label: *mut s::lv_obj_t,
    pub(super) raw: Option<Box<s::lv_image_dsc_t>>,
}
unsafe extern "C" fn deleted(e: *mut s::lv_event_t) {
    unsafe {
        let r = &*(s::lv_event_get_user_data(e) as *const Record);
        r.alive.set(false);
    }
}

impl LuaLvglService {
    pub(super) unsafe fn add(&mut self, p: *mut s::lv_obj_t, kind: &str) -> Result<u64, String> {
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
    pub(super) fn get(&self, id: u64) -> Result<&Record, String> {
        self.objects
            .get(&id)
            .filter(|r| r.alive.get())
            .map(Box::as_ref)
            .ok_or_else(|| "deleted or invalid LVGL object".into())
    }
    pub(super) unsafe fn set_text(&mut self, id: u64, text: &str) -> Result<(), String> {
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
    pub(super) unsafe fn options(&mut self, id: u64, o: &Value) -> Result<(), String> {
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
    pub(super) unsafe fn style(
        &self,
        p: *mut s::lv_obj_t,
        o: &Value,
        options: bool,
    ) -> Result<(), String> {
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
}
