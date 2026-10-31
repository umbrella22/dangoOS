//! Content-keyed icon cache and RGB565 scaling for static LVGL pixels.
use super::ICON_BOX;
use alloc::{boxed::Box, vec::Vec};

/// A launcher icon, RGB565 (little-endian) for LVGL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    pub width: u16,
    pub height: u16,
    pub rgb565: &'static [u8],
}

/// Decoded icons, kept for the firmware's lifetime (LVGL images need
/// `'static` pixels). Keyed by content, so rescans reuse them.
#[derive(Default)]
pub struct IconCache {
    entries: Vec<(u64, Icon)>,
}

/// Most icons kept (each is at most `ICON_BOX`² × 2 bytes = 18 KiB).
const ICON_CACHE_MAX: usize = 48;

impl IconCache {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The icon for this JPEG, decoded and scaled to fit [`ICON_BOX`].
    pub fn get(&mut self, jpeg: &[u8]) -> Option<Icon> {
        let key = fnv1a(jpeg);
        if let Some((_, icon)) = self.entries.iter().find(|(k, _)| *k == key) {
            return Some(*icon);
        }
        if self.entries.len() >= ICON_CACHE_MAX {
            return None;
        }
        let img = lua_engine::jpeg::decode(jpeg).ok()?;
        let icon = scale_to_box(&img);
        self.entries.push((key, icon));
        Some(icon)
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h ^ data.len() as u64
}

/// Nearest-neighbour scale: whole-number enlargement for small (pixel art)
/// icons, shrink to fit for large ones.
pub(super) fn scale_to_box(img: &lua_engine::jpeg::Rgb888) -> Icon {
    let (w, h) = (img.width as u32, img.height as u32);
    let side = w.max(h);
    let (nw, nh) = if side <= ICON_BOX as u32 {
        let k = (ICON_BOX as u32 / side).max(1);
        (w * k, h * k)
    } else {
        (
            (w * ICON_BOX as u32 / side).max(1),
            (h * ICON_BOX as u32 / side).max(1),
        )
    };
    let mut out = Vec::with_capacity((nw * nh * 2) as usize);
    for y in 0..nh {
        let sy = y * h / nh;
        for x in 0..nw {
            let sx = x * w / nw;
            let p = &img.pixels[((sy * w + sx) * 3) as usize..][..3];
            let v = ((p[0] as u16 & 0xF8) << 8) | ((p[1] as u16 & 0xFC) << 3) | (p[2] as u16 >> 3);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    Icon {
        width: nw as u16,
        height: nh as u16,
        rgb565: Box::leak(out.into_boxed_slice()),
    }
}
