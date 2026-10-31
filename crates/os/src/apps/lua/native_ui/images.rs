//! Canvas and image pixel budgets and cache-safe replacement.
use super::{lv_image_cache_drop, values::cstring, LuaLvglService};
use alloc::{boxed::Box, string::String, vec::Vec};
use rs_lvgl::sys as s;
const PIXELS_MAX: usize = 2 * 1024 * 1024;

impl LuaLvglService {
    pub(super) unsafe fn canvas(
        &mut self,
        id: u64,
        input: Option<(Vec<u8>, bool)>,
    ) -> Result<(), String> {
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
    pub(super) unsafe fn raw_image(
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
    pub(super) unsafe fn file_image(&mut self, id: u64, src: &str) -> Result<(), String> {
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
