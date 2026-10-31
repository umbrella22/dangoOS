//! Foreground UI operation dispatch on the single LVGL task.
use super::{
    events::{event, event_code, Event},
    values::*,
    LuaLvglService,
};
use alloc::{boxed::Box, format, rc::Rc, string::String, vec::Vec};
use core::ptr;
use rs_lvgl::sys as s;
use serde_json::{json, Value};

impl LuaLvglService {
    pub(super) unsafe fn dispatch(
        &mut self,
        op: &str,
        a: &Value,
        bytes: Vec<u8>,
    ) -> Result<Value, String> {
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

                    s::lv_obj_delete(p);

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
}
