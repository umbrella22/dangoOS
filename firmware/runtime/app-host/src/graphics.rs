//! Owned framebuffer and fonts backed by the same raster kernels as Lua.
use crate::esp_err;
use alloc::{ffi::CString, vec::Vec};
use core::{
    ffi::{c_char, c_void},
    ptr::NonNull,
};
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
/// Inline dirty regions matching `DISPLAY_DIRTY_RECT_CAPACITY` in display_dirty.h.
/// Copying the publication metadata never allocates or borrows mutable raster state.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rectangles {
    rects: [Rect; 4],
    count: usize,
}
impl core::ops::Deref for Rectangles {
    type Target = [Rect];

    fn deref(&self) -> &[Rect] {
        &self.rects[..self.count]
    }
}
impl<'a> IntoIterator for &'a Rectangles {
    type Item = &'a Rect;
    type IntoIter = core::slice::Iter<'a, Rect>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
#[repr(C)]
struct Raster {
    pixels: *mut u8,
    width: i32,
    height: i32,
    bpp: i32,
    stride: usize,
    tx: i32,
    ty: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    dirty: *mut Rectangles,
}
#[repr(C)]
struct TextOptions {
    font: *mut c_void,
    size: i32,
    color: u32,
}
#[repr(C)]
struct Bitmap {
    pixels: *const u8,
    length: usize,
    stride: usize,
    width: i32,
    height: i32,
    format: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BitmapOptions {
    pub mode: i32,
    pub width: i32,
    pub height: i32,
    pub source_x: i32,
    pub source_y: i32,
    pub source_width: i32,
    pub source_height: i32,
    pub opacity: u8,
}
unsafe extern "C" {
    fn dango_raster_draw(
        r: *mut Raster,
        op: i32,
        args: *const i32,
        color: u32,
        start: f64,
        end: f64,
    );
    fn dango_display_text_measure(
        text: *const c_char,
        length: usize,
        options: *const TextOptions,
        width: *mut i32,
        height: *mut i32,
    ) -> i32;
    fn dango_display_text_draw(
        r: *mut Raster,
        x: i32,
        y: i32,
        text: *const c_char,
        length: usize,
        options: *const TextOptions,
    ) -> i32;
    fn dango_display_bitmap_draw(
        r: *mut Raster,
        x: i32,
        y: i32,
        view: *const Bitmap,
        options: *const BitmapOptions,
    ) -> i32;
    fn dango_font_from_bytes(bytes: *const u8, length: usize, font: *mut *mut c_void) -> i32;
    fn dango_display_font_delete(font: *mut c_void);
    fn dango_format_date(
        seconds: i64,
        format: *const c_char,
        out: *mut c_char,
        cap: usize,
    ) -> usize;
}
fn result(code: i32) -> Result<(), i32> {
    if code == 0 {
        Ok(())
    } else {
        Err(code)
    }
}
pub fn buffer(size: usize) -> Result<Vec<u8>, i32> {
    let mut v = Vec::new();
    v.try_reserve_exact(size).map_err(|_| esp_err::NO_MEM)?;
    v.resize(size, 0);
    Ok(v)
}
pub struct Font(NonNull<c_void>);
impl Font {
    pub fn decode(bytes: &[u8]) -> Result<Self, i32> {
        let mut raw = core::ptr::null_mut();
        result(unsafe { dango_font_from_bytes(bytes.as_ptr(), bytes.len(), &mut raw) })?;
        NonNull::new(raw).map(Self).ok_or(esp_err::NO_MEM)
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        unsafe { dango_display_font_delete(self.0.as_ptr()) }
    }
}
#[derive(Clone, Copy)]
struct State {
    tx: i32,
    ty: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}
pub struct Framebuffer {
    pub width: u16,
    pub height: u16,
    pub count: u8,
    pixels: Vec<u8>,
    other: Vec<u8>,
    state: State,
    stack: Vec<State>,
    dirty: Rectangles,
    active: bool,
    first: bool,
}
impl Framebuffer {
    pub fn new(width: u16, height: u16, count: u8) -> Result<Self, i32> {
        if width == 0 || height == 0 || !(1..=2).contains(&count) {
            return Err(esp_err::INVALID_ARG);
        }
        let size = usize::from(width) * usize::from(height) * 2;
        Ok(Self {
            width,
            height,
            count,
            pixels: buffer(size)?,
            other: if count == 2 {
                buffer(size)?
            } else {
                Vec::new()
            },
            state: State {
                tx: 0,
                ty: 0,
                x0: 0,
                y0: 0,
                x1: i32::from(width),
                y1: i32::from(height),
            },
            stack: Vec::new(),
            dirty: Rectangles::default(),
            active: false,
            first: true,
        })
    }
    pub fn bytes(&self) -> usize {
        self.pixels.len() + self.other.len()
    }
    fn raster(&mut self) -> Raster {
        let s = self.state;
        Raster {
            pixels: self.pixels.as_mut_ptr(),
            width: self.width.into(),
            height: self.height.into(),
            bpp: 2,
            stride: usize::from(self.width) * 2,
            tx: s.tx,
            ty: s.ty,
            x0: s.x0,
            y0: s.y0,
            x1: s.x1,
            y1: s.y1,
            dirty: &mut self.dirty,
        }
    }
    fn active(&self) -> Result<(), i32> {
        if self.active {
            Ok(())
        } else {
            Err(esp_err::INVALID_STATE)
        }
    }
    pub fn begin(&mut self, clear: Option<u32>) -> Result<(), i32> {
        if self.active {
            return Err(esp_err::INVALID_STATE);
        }
        if self.count == 2 {
            self.pixels.copy_from_slice(&self.other)
        }
        self.state = State {
            tx: 0,
            ty: 0,
            x0: 0,
            y0: 0,
            x1: self.width.into(),
            y1: self.height.into(),
        };
        self.stack.clear();
        self.dirty.count = 0;
        self.active = true;
        if let Some(color) = clear {
            self.draw(
                0,
                [0, 0, self.width.into(), self.height.into(), 0, 0],
                color,
                0.,
                0.,
            )?;
        }
        Ok(())
    }
    pub fn save(&mut self) -> Result<(), i32> {
        self.active()?;
        if self.stack.len() == 8 {
            return Err(esp_err::INVALID_STATE);
        }
        self.stack.push(self.state);
        Ok(())
    }
    pub fn restore(&mut self) -> Result<(), i32> {
        self.active()?;
        self.state = self.stack.pop().ok_or(esp_err::INVALID_STATE)?;
        Ok(())
    }
    pub fn translate(&mut self, x: i32, y: i32) -> Result<(), i32> {
        self.active()?;
        let tx = self.state.tx.checked_add(x).ok_or(esp_err::INVALID_ARG)?;
        let ty = self.state.ty.checked_add(y).ok_or(esp_err::INVALID_ARG)?;
        self.state.tx = tx;
        self.state.ty = ty;
        Ok(())
    }
    pub fn clip(&mut self, x: i32, y: i32, w: i32, h: i32) -> Result<(), i32> {
        self.active()?;
        if w < 0 || h < 0 {
            return Err(esp_err::INVALID_ARG);
        }
        let s = &mut self.state;
        let x = i64::from(x) + i64::from(s.tx);
        let y = i64::from(y) + i64::from(s.ty);
        s.x0 = (i64::from(s.x0).max(x)).clamp(0, self.width.into()) as i32;
        s.y0 = (i64::from(s.y0).max(y)).clamp(0, self.height.into()) as i32;
        s.x1 = i64::from(s.x1)
            .min(x + i64::from(w))
            .clamp(s.x0.into(), self.width.into()) as i32;
        s.y1 = i64::from(s.y1)
            .min(y + i64::from(h))
            .clamp(s.y0.into(), self.height.into()) as i32;
        Ok(())
    }
    pub fn draw(
        &mut self,
        op: i32,
        args: [i32; 6],
        color: u32,
        start: f64,
        end: f64,
    ) -> Result<(), i32> {
        self.active()?;
        if !(0..=8).contains(&op) || !start.is_finite() || !end.is_finite() {
            return Err(esp_err::INVALID_ARG);
        }
        unsafe { dango_raster_draw(&mut self.raster(), op, args.as_ptr(), color, start, end) };
        Ok(())
    }
    pub fn measure(text: &str, size: i32, font: Option<&Font>) -> Result<(i32, i32), i32> {
        if font.is_none() && !(8..=64).contains(&size) {
            return Err(esp_err::INVALID_ARG);
        }
        let options = TextOptions {
            font: font.map_or(core::ptr::null_mut(), |f| f.0.as_ptr()),
            size,
            color: u32::MAX,
        };
        let (mut w, mut h) = (0, 0);
        result(unsafe {
            dango_display_text_measure(text.as_ptr().cast(), text.len(), &options, &mut w, &mut h)
        })?;
        Ok((w, h))
    }
    pub fn text(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        size: i32,
        color: u32,
        font: Option<&Font>,
    ) -> Result<(), i32> {
        self.active()?;
        Self::measure(text, size, font)?;
        let options = TextOptions {
            font: font.map_or(core::ptr::null_mut(), |f| f.0.as_ptr()),
            size,
            color,
        };
        result(unsafe {
            dango_display_text_draw(
                &mut self.raster(),
                x,
                y,
                text.as_ptr().cast(),
                text.len(),
                &options,
            )
        })
    }
    pub fn bitmap(
        &mut self,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        format: i32,
        bytes: &[u8],
        options: BitmapOptions,
    ) -> Result<(), i32> {
        self.active()?;
        let bpp = match format {
            0 => 2,
            1 | 2 => 3,
            _ => return Err(esp_err::INVALID_ARG),
        };
        if width == 0
            || height == 0
            || bytes.len() != usize::from(width) * usize::from(height) * bpp
        {
            return Err(esp_err::INVALID_ARG);
        }
        let view = Bitmap {
            pixels: bytes.as_ptr(),
            length: bytes.len(),
            stride: usize::from(width) * bpp,
            width: width.into(),
            height: height.into(),
            format,
        };
        result(unsafe { dango_display_bitmap_draw(&mut self.raster(), x, y, &view, &options) })
    }
    pub fn rectangles(&mut self, full: bool) -> Result<Rectangles, i32> {
        self.active()?;
        if full || self.first {
            self.dirty.rects[0] = Rect {
                x: 0,
                y: 0,
                width: self.width.into(),
                height: self.height.into(),
            };
            self.dirty.count = 1;
        }
        Ok(self.dirty)
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    pub fn presented(&mut self) {
        self.active = false;
        self.first = false;
        if self.count == 2 {
            core::mem::swap(&mut self.pixels, &mut self.other)
        }
    }
}
pub fn date(seconds: i64, format: &str) -> Result<alloc::string::String, i32> {
    if format.len() > 64 {
        return Err(esp_err::INVALID_ARG);
    }
    let format = CString::new(format).map_err(|_| esp_err::INVALID_ARG)?;
    let mut out = [0u8; 128];
    let size =
        unsafe { dango_format_date(seconds, format.as_ptr(), out.as_mut_ptr().cast(), out.len()) };
    if size == 0 {
        return Err(esp_err::INVALID_ARG);
    }
    Ok(core::str::from_utf8(&out[..size])
        .map_err(|_| esp_err::INVALID_ARG)?
        .into())
}
