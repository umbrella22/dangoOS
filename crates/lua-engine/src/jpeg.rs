//! Baseline JPEG decoding (TJpgDec), for app icons such as the factory
//! apps' 48x48 `assets/icon.jpg`.

use alloc::vec::Vec;

extern "C" {
    fn mosaico_jpeg_encode_rgb(
        write: unsafe extern "C" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, i32),
        context: *mut core::ffi::c_void,
        rgb: *const u8,
        width: i32,
        height: i32,
        quality: i32,
    ) -> i32;
    fn mosaico_jpeg_decode(
        data: *const u8,
        len: usize,
        width: *mut u16,
        height: *mut u16,
        out: *mut u8,
        out_len: usize,
    ) -> i32;
}

/// Encode packed RGB888 without a Lua state or VM memory ledger.
pub fn encode_rgb888(width: u16, height: u16, pixels: &[u8], quality: u8) -> Result<Vec<u8>, i32> {
    if width == 0
        || height == 0
        || pixels.len() != usize::from(width) * usize::from(height) * 3
        || !(1..=100).contains(&quality)
    {
        return Err(-1);
    }
    unsafe extern "C" fn write(
        context: *mut core::ffi::c_void,
        data: *mut core::ffi::c_void,
        size: i32,
    ) {
        // SAFETY: the synchronous encoder supplies a live chunk and the
        // exclusive Vec context passed below; it retains neither pointer.
        let out = unsafe { &mut *context.cast::<Vec<u8>>() };
        if size > 0 {
            out.extend_from_slice(unsafe {
                core::slice::from_raw_parts(data.cast::<u8>(), size as usize)
            });
        }
    }
    let mut out = Vec::new();
    // SAFETY: the RGB size was checked and callback/context live for the call.
    let result = unsafe {
        mosaico_jpeg_encode_rgb(
            write,
            (&mut out as *mut Vec<u8>).cast(),
            pixels.as_ptr(),
            i32::from(width),
            i32::from(height),
            i32::from(quality),
        )
    };
    if result == 0 || out.is_empty() {
        Err(-1)
    } else {
        Ok(out)
    }
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
    fn native_encoder_produces_an_album_decodable_jpeg() {
        let rgb = alloc::vec![180u8;16*8*3];
        let encoded = super::encode_rgb888(16, 8, &rgb, 85).unwrap();
        let decoded = super::decode(&encoded).unwrap();
        assert_eq!((decoded.width, decoded.height), (16, 8));
        assert!(decoded
            .pixels
            .iter()
            .all(|&v| (i32::from(v) - 180).abs() <= 3));
        assert!(super::encode_rgb888(16, 8, &rgb[..20], 85).is_err());
        assert!(super::encode_rgb888(16, 8, &rgb, 0).is_err());
    }
    #[test]
    fn decodes_a_baseline_jpeg_and_rejects_garbage() {
        // 16x8 baseline JPEG: left half red, right half blue (made with PIL).
        let jpg = include_bytes!("../test/images/red_blue_16x8.jpg");
        let img = super::decode(jpg).unwrap();
        assert_eq!((img.width, img.height), (16, 8));
        let px = |x: usize, y: usize| &img.pixels[(y * 16 + x) * 3..][..3];
        assert!(px(2, 4)[0] > 200 && px(2, 4)[2] < 60, "{:?}", px(2, 4));
        assert!(px(13, 4)[2] > 200 && px(13, 4)[0] < 60, "{:?}", px(13, 4));
        assert!(super::decode(b"not a jpeg").is_err());
        assert!(super::decode(&jpg[..40]).is_err());
    }
}
