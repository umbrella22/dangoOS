//! Immutable LVGL descriptors for the versioned UI font resource set.
//!
//! Full font data is loaded once into PSRAM by services and activated on the UI
//! thread before rendering. Recovery retains complete 20px text plus subsets
//! at other sizes, so old resource partitions still have a readable UI.

use core::ffi::c_void;
use core::ptr;

use rs_lvgl::sys as lvgl_sys;
use rs_lvgl_ui::Font;

use crate::formats::bitmap_font::FontData;

struct FontSource {
    index: usize,
    recovery: FontData<'static>,
}
impl FontSource {
    fn data(&self) -> FontData<'static> {
        #[cfg(target_os = "none")]
        if self.index == 4 {
            return crate::services::resources::voice_font::active()
                .map_or(self.recovery, |font| font.data());
        }
        #[cfg(target_os = "none")]
        if let Some(fonts) = device::active() {
            return fonts.font(self.index);
        }
        self.recovery
    }
}

/// A font descriptor LVGL may keep a pointer to.
#[repr(transparent)]
pub struct StaticFont(lvgl_sys::lv_font_t);

// SAFETY: the descriptor is immutable after initialization and only points at
// immutable static data; LVGL reads it from the single UI thread.
unsafe impl Sync for StaticFont {}

impl StaticFont {
    /// The `rs_lvgl_ui` handle.
    pub fn font(&'static self) -> Font {
        // SAFETY: `self` is a fully initialized static descriptor.
        unsafe { Font::from_raw(&self.0) }
    }

    const fn new(source: &'static FontSource) -> Self {
        let data = source.recovery;
        // SAFETY: an all-zero `lv_font_t` is valid (null pointers, no
        // callbacks, zero metrics); the fields that matter are set below.
        let zeroed: lvgl_sys::lv_font_t = unsafe { core::mem::zeroed() };
        Self(lvgl_sys::lv_font_t {
            dsc: source as *const FontSource as *const c_void,
            fallback: if source.index == 0 || source.index == 3 {
                ptr::addr_of!(lvgl_sys::lv_font_montserrat_14)
            } else {
                ptr::addr_of!(HUB_20.0)
            },
            get_glyph_dsc: Some(get_glyph_dsc),
            get_glyph_bitmap: Some(get_glyph_bitmap),
            line_height: data.line_height(),
            base_line: data.base_line(),
            ..zeroed
        })
    }
}

/// Resolve a static source through the current immutable font set.
///
/// # Safety
///
/// `font` must point to a live descriptor constructed by this module.
unsafe fn data_of(font: *const lvgl_sys::lv_font_t) -> FontData<'static> {
    // SAFETY: all descriptors using these callbacks point to a static source.
    unsafe { &*((*font).dsc as *const FontSource) }.data()
}

impl StaticFont {
    fn data(&self) -> FontData<'static> {
        // SAFETY: descriptors are constructed only from static sources.
        unsafe { data_of(&self.0) }
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
    let Some(glyph) = data.glyph(index as usize - 1) else {
        return ptr::null();
    };
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

#[cfg(not(target_os = "none"))]
static FILE_20: &[u8] = include_bytes!("../../../assets/hub-20.bin");
#[cfg(not(target_os = "none"))]
static FILE_24: &[u8] = include_bytes!("../../../assets/hub-24.bin");
#[cfg(not(target_os = "none"))]
static FILE_30: &[u8] = include_bytes!("../../../assets/hub-30.bin");
#[cfg(not(target_os = "none"))]
static FILE_DIGITS_64: &[u8] = include_bytes!("../../../assets/hub-digits-64.bin");
#[cfg(target_os = "none")]
static FILE_20: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/recovery-20.bin"));
#[cfg(target_os = "none")]
static FILE_24: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/recovery-24.bin"));
#[cfg(target_os = "none")]
static FILE_30: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/recovery-30.bin"));
#[cfg(target_os = "none")]
static FILE_DIGITS_64: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/recovery-digits-64.bin"));

static SOURCE_20: FontSource = FontSource {
    index: 0,
    recovery: FontData::checked(FILE_20),
};
static SOURCE_24: FontSource = FontSource {
    index: 1,
    recovery: FontData::checked(FILE_24),
};
static SOURCE_30: FontSource = FontSource {
    index: 2,
    recovery: FontData::checked(FILE_30),
};
static SOURCE_DIGITS: FontSource = FontSource {
    index: 3,
    recovery: FontData::checked(FILE_DIGITS_64),
};
static SOURCE_VOICE: FontSource = FontSource {
    index: 4,
    recovery: FontData::checked(FILE_20),
};
/// Arbitrary CJK conversation text; the voice resource is activated by UI.
pub static VOICE_20: StaticFont = StaticFont::new(&SOURCE_VOICE);
/// Body text and small labels.
pub static HUB_20: StaticFont = StaticFont::new(&SOURCE_20);
/// Row labels and values.
pub static HUB_24: StaticFont = StaticFont::new(&SOURCE_24);
/// Page titles (medium weight).
pub static HUB_30: StaticFont = StaticFont::new(&SOURCE_30);
/// The timer's clock digits (`0-9` and `:` only).
pub static HUB_DIGITS_64: StaticFont = StaticFont::new(&SOURCE_DIGITS);

#[cfg(target_os = "none")]
mod device;
#[cfg(target_os = "none")]
pub use device::{activate_prepared, prepare};

pub fn resource_notice() -> Option<&'static str> {
    #[cfg(target_os = "none")]
    if device::recovery_active() {
        return Some("UI fonts need an update. Install resources and restart.");
    }
    None
}

#[cfg(test)]
static ALL: [&StaticFont; 4] = [&HUB_20, &HUB_24, &HUB_30, &HUB_DIGITS_64];

/// Advance width of `ch` in `font`, px (`None` when the glyph is missing and
/// LVGL falls back to Montserrat).
pub fn advance(font: &'static StaticFont, ch: char) -> Option<u16> {
    font.data()
        .find(ch as u32)
        .or_else(|| {
            // Match the descriptor's readable 20px fallback during recovery.
            (font.0.fallback == ptr::addr_of!(HUB_20.0))
                .then(|| HUB_20.data().find(ch as u32))
                .flatten()
        })
        .map(|g| g.adv)
}

/// Whether `font` has a glyph for `ch` (host checks and tests).
pub fn has_glyph(font: &'static StaticFont, ch: char) -> bool {
    font.data().find(ch as u32).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_titles_resolve_missing_cjk_through_the_complete_small_font() {
        static RECOVERY_SOURCE: FontSource = FontSource {
            index: 1,
            recovery: FontData::checked(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/recovery-24.bin"
            ))),
        };
        static RECOVERY_FONT: StaticFont = StaticFont::new(&RECOVERY_SOURCE);
        assert!(RECOVERY_SOURCE.recovery.find('设' as u32).is_none());
        let mut glyph: lvgl_sys::lv_font_glyph_dsc_t = unsafe { core::mem::zeroed() };
        // SAFETY: all pointers refer to live static descriptors/output. LVGL's
        // font lookup only traverses the immutable chain, without UI objects.
        assert!(unsafe {
            lvgl_sys::lv_font_get_glyph_dsc(&RECOVERY_FONT.0, &mut glyph, '设' as u32, 0)
        });
        assert_eq!(glyph.resolved_font, ptr::addr_of!(HUB_20.0));
        assert_eq!(glyph.adv_w, advance(&HUB_20, '设').unwrap());
    }

    #[test]
    fn recovery_fonts_keep_exact_metrics_rasters_and_terminal_battery_copy() {
        let recovery: [&[u8]; 4] = [
            include_bytes!(concat!(env!("OUT_DIR"), "/recovery-20.bin")),
            include_bytes!(concat!(env!("OUT_DIR"), "/recovery-24.bin")),
            include_bytes!(concat!(env!("OUT_DIR"), "/recovery-30.bin")),
            include_bytes!(concat!(env!("OUT_DIR"), "/recovery-digits-64.bin")),
        ];
        for (i, bytes) in recovery.into_iter().enumerate() {
            let small = FontData::parse(bytes).unwrap();
            let full = ALL[i].data();
            assert_eq!(
                (small.line_height(), small.base_line()),
                (full.line_height(), full.base_line())
            );
            for n in 0..small.count() {
                let g = small.glyph(n).unwrap();
                let original = full.find(small.code_point(n)).unwrap();
                assert_eq!(
                    (g.adv, g.w, g.h, g.ofs_x, g.ofs_y),
                    (
                        original.adv,
                        original.w,
                        original.h,
                        original.ofs_x,
                        original.ofs_y
                    )
                );
                assert_eq!(small.rows(g), full.rows(original));
            }
            if i < 3 {
                for ch in "低电量即将关机，请连接电源".chars() {
                    assert!(small.find(ch as u32).is_some());
                }
            }
        }
    }

    #[test]
    fn the_files_parse_and_are_sorted() {
        for font in ALL {
            let data = font.data();
            assert!(data.count() > 0);
            for i in 1..data.count() {
                assert!(
                    data.code_point(i - 1) < data.code_point(i),
                    "sorted, unique"
                );
            }
            let last = data.glyph(data.count() - 1).unwrap();
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
            include_str!("../../pages/launcher.rs"),
            include_str!("../../pages/sensors.rs"),
            include_str!("../../pages/timer.rs"),
            include_str!("../../pages/camera.rs"),
            include_str!("../../pages/voice.rs"),
            include_str!("../../pages/lua.rs"),
            include_str!("../../pages/low_battery.rs"),
            include_str!("../../components/page.rs"),
            include_str!("../../pages/settings/update.rs"),
            include_str!("../../pages/settings/view/mod.rs"),
            include_str!("../../pages/settings/view/audio.rs"),
            include_str!("../../pages/settings/view/camera.rs"),
            include_str!("../../pages/settings/view/categories.rs"),
            include_str!("../../pages/settings/view/character.rs"),
            include_str!("../../pages/settings/view/components.rs"),
            include_str!("../../pages/settings/view/display.rs"),
            include_str!("../../pages/settings/view/feedback.rs"),
            include_str!("../../pages/settings/view/password.rs"),
            include_str!("../../pages/settings/view/proxy.rs"),
            include_str!("../../pages/settings/view/storage.rs"),
            include_str!("../../pages/settings/view/system.rs"),
            include_str!("../../pages/settings/view/update.rs"),
            include_str!("../../pages/settings/view/wifi.rs"),
            include_str!("../../pages/settings/view/wireless.rs"),
            include_str!("../../app/mod.rs"),
            include_str!("../../pages/settings/model.rs"),
            include_str!("../../runtime/mod.rs"),
            include_str!("../../runtime/settings.rs"),
            include_str!("../../../services/radio/mod.rs"),
            include_str!("../../../services/voice/config.rs"),
            include_str!("../../../services/voice/device.rs"),
            include_str!("../../../services/resources/voice_font.rs"),
            include_str!("../../../services/camera/mod.rs"),
            include_str!("../../model.rs"),
            include_str!("../../../services/timer.rs"),
            include_str!("../../../services/input/snapshot.rs"),
            include_str!("../../navigation/gesture.rs"),
        ];
        for source in sources {
            for literal in source.split('"').skip(1).step_by(2) {
                for ch in literal.chars().filter(|&c| {
                    ('\u{4E00}'..='\u{9FFF}').contains(&c)
                        || ('\u{3000}'..='\u{303F}').contains(&c)
                        || ('\u{FF00}'..='\u{FFEF}').contains(&c)
                }) {
                    for font in [&HUB_20, &HUB_24, &HUB_30] {
                        assert!(
                            has_glyph(font, ch),
                            "{ch} is missing: rerun scripts/assets/gen_hub_font.py"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn filesystem_app_titles_have_cjk_glyphs_in_every_text_size() {
        fn check_titles(root: &std::path::Path) {
            for entry in std::fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    check_titles(&path);
                } else if path.file_name().is_some_and(|name| name == "launcher.json") {
                    let manifest: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    for ch in manifest["display_name"]
                        .as_str()
                        .unwrap()
                        .chars()
                        .filter(|c| !c.is_ascii())
                    {
                        for font in [&HUB_20, &HUB_24, &HUB_30] {
                            assert!(
                                has_glyph(font, ch),
                                "{ch} in {} has no glyph",
                                path.display()
                            );
                        }
                    }
                }
            }
        }
        check_titles(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps"));
    }
}
