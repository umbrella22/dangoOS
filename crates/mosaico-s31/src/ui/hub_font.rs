//! The hub's CJK bitmap fonts (RFC 0006 section 2.2).
//!
//! `assets/fonts/gen_hub_font.py` rasterizes the characters the hub sources
//! use from Noto Sans SC into `src/assets/hub-*.bin` (4 bpp, format in the
//! script's docstring). This module wraps each file in a static `lv_font_t`
//! whose glyph callbacks are written here, so no C font file and no LVGL font
//! loader is needed.
//!
//! Missing glyphs fall back to Montserrat 14 (LVGL's default), which also
//! carries the `LV_SYMBOL_*` icons.

use core::ffi::c_void;
use core::ptr;

use lvgl::sys as lvgl_sys;
use lvgl_ui::Font;

const HEADER: usize = 16;
const ENTRY: usize = 16;

/// One generated font file.
#[derive(Clone, Copy)]
struct FontData(&'static [u8]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Glyph {
    index: u32,
    offset: u32,
    adv: u16,
    w: u8,
    h: u8,
    ofs_x: i8,
    ofs_y: i8,
}

const fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

const fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

impl FontData {
    const fn check(self) -> Self {
        let d = self.0;
        assert!(d.len() >= HEADER);
        assert!(d[0] == b'M' && d[1] == b'F' && d[2] == b'N' && d[3] == b'T');
        assert!(d[4] == 1, "font file version");
        assert!(d[5] == 4, "4 bpp");
        assert!(d.len() >= HEADER + ENTRY * u16_at(d, 10) as usize);
        self
    }

    const fn line_height(self) -> i32 {
        u16_at(self.0, 6) as i32
    }

    const fn base_line(self) -> i32 {
        u16_at(self.0, 8) as i32
    }

    fn count(self) -> usize {
        if self.0.len() < HEADER {
            return 0;
        }
        u16_at(self.0, 10) as usize
    }

    fn bitmaps(self) -> &'static [u8] {
        self.0.get(HEADER + ENTRY * self.count()..).unwrap_or(&[])
    }

    fn glyph(self, index: usize) -> Glyph {
        let at = HEADER + ENTRY * index;
        let d = self.0;
        Glyph {
            index: index as u32,
            offset: u32_at(d, at + 4),
            adv: u16_at(d, at + 8),
            w: d[at + 10],
            h: d[at + 11],
            ofs_x: d[at + 12] as i8,
            ofs_y: d[at + 13] as i8,
        }
    }

    fn code_point(self, index: usize) -> u32 {
        u32_at(self.0, HEADER + ENTRY * index)
    }

    fn find(self, letter: u32) -> Option<Glyph> {
        let (mut lo, mut hi) = (0usize, self.count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.code_point(mid).cmp(&letter) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => return Some(self.glyph(mid)),
            }
        }
        None
    }

    fn rows(self, glyph: Glyph) -> &'static [u8] {
        let stride = (glyph.w as usize).div_ceil(2);
        let start = glyph.offset as usize;
        &self.bitmaps()[start..start + stride * glyph.h as usize]
    }
}

/// A font descriptor LVGL may keep a pointer to.
#[repr(transparent)]
pub struct StaticFont(lvgl_sys::lv_font_t);

// SAFETY: the descriptor is immutable after initialization and only points at
// immutable static data; LVGL reads it from the single UI thread.
unsafe impl Sync for StaticFont {}

impl StaticFont {
    /// The `lvgl-ui` handle.
    pub fn font(&'static self) -> Font {
        // SAFETY: `self` is a fully initialized static descriptor.
        unsafe { Font::from_raw(&self.0) }
    }

    const fn new(data: &'static [u8]) -> Self {
        let data = FontData(data).check();
        // SAFETY: an all-zero `lv_font_t` is valid (null pointers, no
        // callbacks, zero metrics); the fields that matter are set below.
        let zeroed: lvgl_sys::lv_font_t = unsafe { core::mem::zeroed() };
        Self(lvgl_sys::lv_font_t {
            dsc: data.0.as_ptr() as *const c_void,
            fallback: ptr::addr_of!(lvgl_sys::lv_font_montserrat_14),
            get_glyph_dsc: Some(get_glyph_dsc),
            get_glyph_bitmap: Some(get_glyph_bitmap),
            line_height: data.line_height(),
            base_line: data.base_line(),
            ..zeroed
        })
    }
}

/// The file behind a descriptor: `dsc` is the start of one of the files.
///
/// # Safety
///
/// `font` must point to a live `lv_font_t`.
unsafe fn data_of(font: *const lvgl_sys::lv_font_t) -> FontData {
    file_at(unsafe { (*font).dsc } as *const u8)
}

fn file_at(dsc: *const u8) -> FontData {
    for file in FILES {
        if file.as_ptr() == dsc {
            return FontData(file);
        }
    }
    FontData(EMPTY)
}

impl StaticFont {
    fn data(&self) -> FontData {
        file_at(self.0.dsc as *const u8)
    }
}

unsafe extern "C" fn get_glyph_dsc(
    font: *const lvgl_sys::lv_font_t,
    out: *mut lvgl_sys::lv_font_glyph_dsc_t,
    letter: u32,
    _letter_next: u32,
) -> bool {
    if font.is_null() || out.is_null() {
        return false;
    }
    // SAFETY: LVGL passes the descriptor these callbacks are installed on.
    let data = unsafe { data_of(font) };
    let Some(glyph) = data.find(letter) else {
        return false;
    };
    // SAFETY: `out` is LVGL's live descriptor for this lookup.
    let out = unsafe { &mut *out };
    out.adv_w = glyph.adv;
    out.box_w = glyph.w as u16;
    out.box_h = glyph.h as u16;
    out.ofs_x = glyph.ofs_x as i16;
    out.ofs_y = glyph.ofs_y as i16;
    out.stride = (glyph.w as u16).div_ceil(2);
    out.format = lvgl_sys::lv_font_glyph_format_t_LV_FONT_GLYPH_FORMAT_A4;
    out.set_is_placeholder(0);
    // 0 means "no glyph" to LVGL's cache, so the index is shifted by one.
    out.gid.index = glyph.index + 1;
    true
}

/// LVGL's 4-bit to 8-bit opacity mapping.
const OPA4: [u8; 16] = [
    0, 17, 34, 51, 68, 85, 102, 119, 136, 153, 170, 187, 204, 221, 238, 255,
];

unsafe extern "C" fn get_glyph_bitmap(
    g_dsc: *mut lvgl_sys::lv_font_glyph_dsc_t,
    draw_buf: *mut lvgl_sys::lv_draw_buf_t,
) -> *const c_void {
    if g_dsc.is_null() {
        return ptr::null();
    }
    // SAFETY: LVGL passes the descriptor filled by `get_glyph_dsc`.
    let g_dsc = unsafe { &mut *g_dsc };
    // SAFETY: the glyph id is the only union member these fonts use.
    let index = unsafe { g_dsc.gid.index };
    if index == 0 || g_dsc.resolved_font.is_null() {
        return ptr::null();
    }
    // SAFETY: `resolved_font` is the font that answered `get_glyph_dsc`.
    let data = unsafe { data_of(g_dsc.resolved_font) };
    let glyph = data.glyph(index as usize - 1);
    if glyph.w == 0 || glyph.h == 0 {
        return ptr::null();
    }
    let rows = data.rows(glyph);
    if g_dsc.req_raw_bitmap() != 0 {
        return rows.as_ptr() as *const c_void;
    }
    if draw_buf.is_null() {
        return ptr::null();
    }
    let (w, h) = (glyph.w as usize, glyph.h as usize);
    // SAFETY: LVGL sizes the scratch buffer for an A8 glyph of this box.
    let out = unsafe {
        let stride = lvgl_sys::lv_draw_buf_width_to_stride(
            w as u32,
            lvgl_sys::lv_color_format_t_LV_COLOR_FORMAT_A8,
        ) as usize;
        let out = core::slice::from_raw_parts_mut((*draw_buf).data, stride * h);
        (out, stride)
    };
    let (out, stride_out) = out;
    let stride_in = w.div_ceil(2);
    for y in 0..h {
        let src = &rows[y * stride_in..(y + 1) * stride_in];
        let dst = &mut out[y * stride_out..y * stride_out + w];
        for (x, pixel) in dst.iter_mut().enumerate() {
            let byte = src[x / 2];
            let nibble = if x % 2 == 0 { byte >> 4 } else { byte & 0x0F };
            *pixel = OPA4[nibble as usize];
        }
    }
    draw_buf as *const c_void
}

static EMPTY: &[u8] = &[];
static FILE_20: &[u8] = include_bytes!("../assets/hub-20.bin");
static FILE_24: &[u8] = include_bytes!("../assets/hub-24.bin");
static FILE_30: &[u8] = include_bytes!("../assets/hub-30.bin");
static FILE_DIGITS_64: &[u8] = include_bytes!("../assets/hub-digits-64.bin");

/// Body text and small labels.
pub static HUB_20: StaticFont = StaticFont::new(FILE_20);
/// Row labels and values.
pub static HUB_24: StaticFont = StaticFont::new(FILE_24);
/// Page titles (medium weight).
pub static HUB_30: StaticFont = StaticFont::new(FILE_30);
/// The timer's clock digits (`0-9` and `:` only).
pub static HUB_DIGITS_64: StaticFont = StaticFont::new(FILE_DIGITS_64);

static FILES: [&[u8]; 4] = [FILE_20, FILE_24, FILE_30, FILE_DIGITS_64];
#[cfg(test)]
static ALL: [&StaticFont; 4] = [&HUB_20, &HUB_24, &HUB_30, &HUB_DIGITS_64];

/// Whether `font` has a glyph for `ch` (host checks and tests).
pub fn has_glyph(font: &'static StaticFont, ch: char) -> bool {
    font.data().find(ch as u32).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_parse_and_are_sorted() {
        for font in ALL {
            let data = font.data();
            assert!(data.count() > 0);
            for i in 1..data.count() {
                assert!(data.code_point(i - 1) < data.code_point(i), "sorted, unique");
            }
            let last = data.glyph(data.count() - 1);
            assert!(
                last.offset as usize + (last.w as usize).div_ceil(2) * last.h as usize
                    <= data.bitmaps().len(),
                "bitmaps fit the file"
            );
            assert!(font.0.line_height > 0 && font.0.base_line >= 0);
        }
    }

    #[test]
    fn text_fonts_cover_ascii_and_digits_font_covers_the_clock() {
        for ch in (0x20u8..0x7F).map(char::from) {
            assert!(has_glyph(&HUB_24, ch), "{ch:?}");
        }
        for ch in "0123456789:".chars() {
            assert!(has_glyph(&HUB_DIGITS_64, ch));
        }
        assert!(!has_glyph(&HUB_DIGITS_64, 'A'));
    }

    /// Every CJK character in a hub string literal is in every text font: the
    /// generator was rerun after the last copy change.
    #[test]
    fn every_hub_string_has_glyphs() {
        let sources = [
            include_str!("hub_view.rs"),
            include_str!("../hub/mod.rs"),
            include_str!("../hub/countdown.rs"),
            include_str!("../hub/snapshot.rs"),
            include_str!("../hub/gesture.rs"),
        ];
        for source in sources {
            for literal in source.split('"').skip(1).step_by(2) {
                for ch in literal.chars().filter(|&c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3000}'..='\u{303F}').contains(&c) || ('\u{FF00}'..='\u{FFEF}').contains(&c)) {
                    for font in [&HUB_20, &HUB_24, &HUB_30] {
                        assert!(
                            has_glyph(font, ch),
                            "{ch} is missing: rerun assets/fonts/gen_hub_font.py"
                        );
                    }
                }
            }
        }
    }
}
