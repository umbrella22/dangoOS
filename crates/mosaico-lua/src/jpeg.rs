//! Baseline JPEG decoding (TJpgDec), for app icons such as the factory
//! apps' 48x48 `assets/icon.jpg`.

use alloc::vec::Vec;

extern "C" {
    fn mosaico_jpeg_decode(
        data: *const u8,
        len: usize,
        width: *mut u16,
        height: *mut u16,
        out: *mut u8,
        out_len: usize,
    ) -> i32;
}

/// A decoded image, RGB888 rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgb888 {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
}

/// Largest image accepted, in pixels per side (bounds the allocation).
pub const MAX_SIDE: u16 = 512;

/// Decodes a baseline JPEG. Errors carry the TJpgDec result code; progressive
/// JPEGs are not supported.
pub fn decode(data: &[u8]) -> Result<Rgb888, i32> {
    let (mut width, mut height) = (0u16, 0u16);
    // SAFETY: the C side reads `len` bytes of `data` and writes the sizes.
    let r = unsafe {
        mosaico_jpeg_decode(
            data.as_ptr(),
            data.len(),
            &mut width,
            &mut height,
            core::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return Err(r);
    }
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(-1);
    }
    let mut pixels = alloc::vec![0u8; width as usize * height as usize * 3];
    // SAFETY: `pixels` holds width * height * 3 bytes, as the C side checks.
    let r = unsafe {
        mosaico_jpeg_decode(
            data.as_ptr(),
            data.len(),
            &mut width,
            &mut height,
            pixels.as_mut_ptr(),
            pixels.len(),
        )
    };
    if r != 0 {
        return Err(r);
    }
    // LVGL's TJpgDec writes B, G, R (LVGL's RGB888 byte order).
    for px in pixels.chunks_exact_mut(3) {
        px.swap(0, 2);
    }
    Ok(Rgb888 {
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_a_baseline_jpeg_and_rejects_garbage() {
        // 16x8 baseline JPEG: left half red, right half blue (made with PIL).
        let jpg = include_bytes!("../testdata/red_blue_16x8.jpg");
        let img = super::decode(jpg).unwrap();
        assert_eq!((img.width, img.height), (16, 8));
        let px = |x: usize, y: usize| &img.pixels[(y * 16 + x) * 3..][..3];
        assert!(px(2, 4)[0] > 200 && px(2, 4)[2] < 60, "{:?}", px(2, 4));
        assert!(px(13, 4)[2] > 200 && px(13, 4)[0] < 60, "{:?}", px(13, 4));
        assert!(super::decode(b"not a jpeg").is_err());
        assert!(super::decode(&jpg[..40]).is_err());
    }
}
