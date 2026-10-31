//! SC101IOT/OV3640 UYVY decoding. BT.601 limited-range video levels.
use alloc::vec::Vec;
pub const PREVIEW_WIDTH: usize = 360;
pub const PREVIEW_HEIGHT: usize = 270;

pub fn rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = i32::from(y) - 16;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    [
        ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8,
        ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8,
        ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8,
    ]
}
pub fn pixel(source: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let at = (y * width + (x & !1)) * 2;
    rgb(source[at + 1 + (x & 1) * 2], source[at], source[at + 2])
}
pub fn preview_row(
    source: &[u8],
    width: usize,
    height: usize,
    y: usize,
    out: &mut [u16],
    mirror: bool,
) {
    for (x, value) in out.iter_mut().enumerate() {
        let source_x = x * width / PREVIEW_WIDTH;
        let [r, g, b] = pixel(
            source,
            width,
            if mirror {
                width - 1 - source_x
            } else {
                source_x
            },
            y * height / PREVIEW_HEIGHT,
        );
        *value = ((u16::from(r) >> 3) << 11) | ((u16::from(g) >> 2) << 5) | (u16::from(b) >> 3);
    }
}
pub fn rgb888(
    source: &[u8],
    width: usize,
    height: usize,
    mirror: bool,
) -> Result<Vec<u8>, &'static str> {
    let pixels = width.checked_mul(height).ok_or("image-size")?;
    let rgb_bytes = pixels.checked_mul(3).ok_or("image-size")?;
    if width == 0
        || height == 0
        || width % 2 != 0
        || source.len() != pixels.checked_mul(2).ok_or("image-size")?
    {
        return Err("image-size");
    }
    let mut out = Vec::with_capacity(rgb_bytes);
    for y in 0..height {
        for x in 0..width {
            out.extend_from_slice(&pixel(
                source,
                width,
                if mirror { width - 1 - x } else { x },
                y,
            ));
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_levels_and_shared_chroma_are_decoded() {
        assert_eq!(rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(rgb(235, 128, 128), [255, 255, 255]);
        assert_eq!(rgb(81, 90, 240), [255, 0, 0]);
        assert_eq!(
            rgb888(&[128, 16, 128, 235], 2, 1, false).unwrap(),
            [0, 0, 0, 255, 255, 255]
        );
        assert_eq!(
            rgb888(&[128, 16, 128, 235], 2, 1, true).unwrap(),
            [255, 255, 255, 0, 0, 0]
        );
        assert!(rgb888(&[0; 3], 2, 1, false).is_err());
        assert!(rgb888(&[0; 6], 3, 1, false).is_err());
        assert!(rgb888(&[], 2, 0, false).is_err());
        assert!(rgb888(&[], usize::MAX, 2, false).is_err());
        let mut row = [0u16; PREVIEW_WIDTH];
        preview_row(&[128, 16, 128, 235], 2, 1, 0, &mut row, false);
        assert_eq!((row[0], row[PREVIEW_WIDTH - 1]), (0, 0xffff));
        preview_row(&[128, 16, 128, 235], 2, 1, 0, &mut row, true);
        assert_eq!((row[0], row[PREVIEW_WIDTH - 1]), (0xffff, 0));
    }
}
