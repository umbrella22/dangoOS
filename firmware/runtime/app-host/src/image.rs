//! Application image frames; formats and bilinear rounding match Lua image.
use crate::{esp_err, graphics::buffer};
use alloc::vec::Vec;
pub const FILE_MAX: usize = 4 * 1024 * 1024;
pub const PIXELS_MAX: usize = 1920 * 1080;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgb565,
    Rgb565Be,
    Rgb888,
    Bgr888,
    Gray8,
    Jpeg,
    Mjpeg,
}
impl Format {
    pub fn parse(s: &str) -> Result<Self, i32> {
        match s {
            "RGBP" | "rgb565" => Ok(Self::Rgb565),
            "RGBR" | "rgb565be" => Ok(Self::Rgb565Be),
            "RGB3" | "rgb888" => Ok(Self::Rgb888),
            "BGR3" | "bgr888" => Ok(Self::Bgr888),
            "GREY" | "gray8" => Ok(Self::Gray8),
            "JPEG" | "jpeg" => Ok(Self::Jpeg),
            "MJPG" | "mjpeg" => Ok(Self::Mjpeg),
            _ => Err(esp_err::NOT_SUPPORTED),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Rgb565 => "RGBP",
            Self::Rgb565Be => "RGBR",
            Self::Rgb888 => "RGB3",
            Self::Bgr888 => "BGR3",
            Self::Gray8 => "GREY",
            Self::Jpeg => "JPEG",
            Self::Mjpeg => "MJPG",
        }
    }
    fn bpp(self) -> usize {
        match self {
            Self::Rgb888 | Self::Bgr888 => 3,
            Self::Gray8 => 1,
            _ => 2,
        }
    }
}
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub format: Format,
    pub pixels: Vec<u8>,
}
unsafe extern "C" {
    fn dango_png(
        input: *const u8,
        len: usize,
        pixels: *mut *mut u8,
        w: *mut u32,
        h: *mut u32,
    ) -> i32;
    fn dango_png_free(pixels: *mut u8);
    fn dango_resize(source: *const u8, sw: u32, sh: u32, out: *mut u8, w: u32, h: u32, gray: i32);
}
fn dimensions(w: u16, h: u16) -> Result<usize, i32> {
    let n = usize::from(w) * usize::from(h);
    if w == 0 || h == 0 || n > PIXELS_MAX {
        Err(esp_err::INVALID_ARG)
    } else {
        Ok(n)
    }
}
fn copy(bytes: &[u8]) -> Result<Vec<u8>, i32> {
    let mut out = buffer(bytes.len())?;
    out.copy_from_slice(bytes);
    Ok(out)
}
impl Frame {
    pub fn raw(w: u16, h: u16, format: Format, pixels: Vec<u8>) -> Result<Self, i32> {
        let n = dimensions(w, h)?;
        if matches!(format, Format::Jpeg | Format::Mjpeg) {
            if pixels.is_empty()
                || pixels.len() > FILE_MAX
                || crate::jpeg::dimensions(&pixels)? != (w, h)
            {
                return Err(esp_err::INVALID_ARG);
            }
        } else if pixels.len() != n * format.bpp() {
            return Err(esp_err::INVALID_ARG);
        }
        Ok(Self {
            width: w,
            height: h,
            format,
            pixels,
        })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.is_empty() || bytes.len() > FILE_MAX {
            return Err(esp_err::INVALID_ARG);
        }
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            if bytes.len() < 24 {
                return Err(esp_err::INVALID_ARG);
            }
            let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
            dimensions(
                u16::try_from(w).map_err(|_| esp_err::INVALID_ARG)?,
                u16::try_from(h).map_err(|_| esp_err::INVALID_ARG)?,
            )?;
            let (mut width, mut height, mut ptr) = (0, 0, core::ptr::null_mut());
            let code = unsafe {
                dango_png(
                    bytes.as_ptr(),
                    bytes.len(),
                    &mut ptr,
                    &mut width,
                    &mut height,
                )
            };
            if code != 0 || ptr.is_null() {
                if !ptr.is_null() {
                    unsafe { dango_png_free(ptr) }
                }
                return Err(esp_err::INVALID_ARG);
            }
            let result = (|| {
                let n = width as usize * height as usize;
                let rgba = unsafe { core::slice::from_raw_parts(ptr, n * 4) };
                let mut rgb = buffer(n * 3)?;
                for (to, from) in rgb.chunks_exact_mut(3).zip(rgba.chunks_exact(4)) {
                    for c in 0..3 {
                        to[c] = ((u32::from(from[c]) * u32::from(from[3]) + 127) / 255) as u8
                    }
                }
                Self::raw(width as u16, height as u16, Format::Rgb888, rgb)
            })();
            unsafe { dango_png_free(ptr) }
            result
        } else {
            let (w, h) = crate::jpeg::dimensions(bytes)?;
            Self::raw(w, h, Format::Jpeg, copy(bytes)?)
        }
    }
    fn rgb(&self) -> Result<Vec<u8>, i32> {
        if matches!(self.format, Format::Jpeg | Format::Mjpeg) {
            return Ok(crate::jpeg::decode_limited(&self.pixels, PIXELS_MAX, None)?.pixels);
        }
        let n = dimensions(self.width, self.height)?;
        let mut out = buffer(n * 3)?;
        for i in 0..n {
            let (r, g, b) = match self.format {
                Format::Rgb888 => (
                    self.pixels[3 * i],
                    self.pixels[3 * i + 1],
                    self.pixels[3 * i + 2],
                ),
                Format::Bgr888 => (
                    self.pixels[3 * i + 2],
                    self.pixels[3 * i + 1],
                    self.pixels[3 * i],
                ),
                Format::Gray8 => (self.pixels[i], self.pixels[i], self.pixels[i]),
                Format::Rgb565 | Format::Rgb565Be => {
                    let pair = [self.pixels[2 * i], self.pixels[2 * i + 1]];
                    let v = if self.format == Format::Rgb565 {
                        u16::from_le_bytes(pair)
                    } else {
                        u16::from_be_bytes(pair)
                    };
                    (
                        (((v >> 11) & 31) * 255 / 31) as u8,
                        (((v >> 5) & 63) * 255 / 63) as u8,
                        ((v & 31) * 255 / 31) as u8,
                    )
                }
                _ => unreachable!(),
            };
            out[3 * i..3 * i + 3].copy_from_slice(&[r, g, b]);
        }
        Ok(out)
    }
    pub fn convert(&self, format: Format) -> Result<Self, i32> {
        if format == self.format {
            return Self::raw(self.width, self.height, format, copy(&self.pixels)?);
        }
        let rgb = self.rgb()?;
        let pixels = if matches!(format, Format::Jpeg | Format::Mjpeg) {
            let out = crate::jpeg::encode_rgb888(self.width, self.height, &rgb, 90)?;
            if out.len() > FILE_MAX {
                return Err(esp_err::NO_MEM);
            }
            out
        } else {
            let mut out =
                buffer(usize::from(self.width) * usize::from(self.height) * format.bpp())?;
            for (to, from) in out.chunks_exact_mut(format.bpp()).zip(rgb.chunks_exact(3)) {
                pixel(to, format, from[0], from[1], from[2]);
            }
            out
        };
        Self::raw(self.width, self.height, format, pixels)
    }
    pub fn resize(&self, w: u16, h: u16, format: Format, bilinear: bool) -> Result<Self, i32> {
        if !matches!(format, Format::Rgb565 | Format::Gray8) {
            return Err(esp_err::NOT_SUPPORTED);
        }
        let n = dimensions(w, h)?;
        let rgb = self.rgb()?;
        let mut out = buffer(n * format.bpp())?;
        if bilinear && (w != self.width || h != self.height) {
            unsafe {
                dango_resize(
                    rgb.as_ptr(),
                    self.width.into(),
                    self.height.into(),
                    out.as_mut_ptr(),
                    w.into(),
                    h.into(),
                    (format == Format::Gray8) as i32,
                )
            }
        } else {
            for y in 0..usize::from(h) {
                for x in 0..usize::from(w) {
                    let i = (y * usize::from(self.height) / usize::from(h)
                        * usize::from(self.width)
                        + x * usize::from(self.width) / usize::from(w))
                        * 3;
                    let at = (y * usize::from(w) + x) * format.bpp();
                    pixel(&mut out[at..], format, rgb[i], rgb[i + 1], rgb[i + 2]);
                }
            }
        }
        Self::raw(w, h, format, out)
    }
}
fn pixel(out: &mut [u8], format: Format, r: u8, g: u8, b: u8) {
    match format {
        Format::Gray8 => {
            out[0] = ((77 * u32::from(r) + 150 * u32::from(g) + 29 * u32::from(b) + 128) >> 8) as u8
        }
        Format::Rgb888 => out[..3].copy_from_slice(&[r, g, b]),
        Format::Bgr888 => out[..3].copy_from_slice(&[b, g, r]),
        _ => {
            let v = ((u16::from(r) >> 3) << 11) | ((u16::from(g) >> 2) << 5) | (u16::from(b) >> 3);
            out[..2].copy_from_slice(&if format == Format::Rgb565Be {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            });
        }
    }
}
