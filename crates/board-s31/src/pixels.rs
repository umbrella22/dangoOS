//! Explicit pixel storage and bounded transfers shared by the panel and host simulator.

use crate::geometry::DirtyRect;
#[path = "display/packing.rs"]
mod packing;

/// S31 SPI DMA's maximum payload per transfer in the locked esp-hal revision.
pub const MAX_DMA_BYTES: usize = 32_736;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb565Le,
    Rgb565Be,
    /// LVGL RGB888 storage: blue, green, red. Panel MADCTL must select BGR.
    Bgr888,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb565Le | Self::Rgb565Be => 2,
            Self::Bgr888 => 3,
        }
    }
}

#[cfg(feature = "display-rgb888")]
pub const PANEL_FORMAT: PixelFormat = PixelFormat::Bgr888;
#[cfg(not(feature = "display-rgb888"))]
pub const PANEL_FORMAT: PixelFormat = PixelFormat::Rgb565Be;

#[derive(Clone, Copy)]
enum Storage<'a> {
    Native565(&'a [u16]),
    Bytes(&'a [u8], PixelFormat),
}

/// Immutable pixels. A source view borrows its original allocation throughout DMA.
#[derive(Clone, Copy)]
pub struct PixelSource<'a> {
    storage: Storage<'a>,
    size: (u16, u16),
    stride: usize,
}

impl<'a> PixelSource<'a> {
    pub fn rgb565(pixels: &'a [u16], size: (u16, u16)) -> Self {
        assert!(size.0 > 0 && size.1 > 0);
        assert!(pixels.len() >= size.0 as usize * size.1 as usize);
        Self {
            storage: Storage::Native565(pixels),
            size,
            stride: size.0 as usize * 2,
        }
    }

    pub fn bytes(bytes: &'a [u8], size: (u16, u16), stride: usize, format: PixelFormat) -> Self {
        assert!(size.0 > 0 && size.1 > 0);
        let row_bytes = size.0 as usize * format.bytes_per_pixel();
        assert!(stride >= row_bytes, "pixel stride shorter than a row");
        let required = (size.1 as usize - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row_bytes))
            .expect("pixel layout overflow");
        assert!(
            bytes.len() >= required,
            "pixel source shorter than its layout"
        );
        Self {
            storage: Storage::Bytes(bytes, format),
            size,
            stride,
        }
    }

    pub const fn size(self) -> (u16, u16) {
        self.size
    }

    fn validate(self, area: DirtyRect) {
        assert!(
            !area.is_empty() && area.x1 < self.size.0 && area.y1 < self.size.1,
            "pixel window outside source"
        );
    }

    /// Exact wire bytes when no conversion or row-gap removal is necessary.
    /// A one-row subwindow is contiguous even when the parent has padding.
    pub fn contiguous(self, area: DirtyRect, format: PixelFormat) -> Option<&'a [u8]> {
        self.validate(area);
        let Storage::Bytes(bytes, stored) = self.storage else {
            return None;
        };
        let row_bytes = area.width() * format.bytes_per_pixel();
        if stored != format || (area.height() > 1 && row_bytes != self.stride) {
            return None;
        }
        let start = area.y0 as usize * self.stride + area.x0 as usize * format.bytes_per_pixel();
        Some(&bytes[start..start + row_bytes * area.height()])
    }

    /// Encode only the requested pixels, excluding source padding.
    pub fn pack(self, area: DirtyRect, format: PixelFormat, output: &mut [u8]) -> usize {
        self.validate(area);
        let row_bytes = area.width() * format.bytes_per_pixel();
        let length = row_bytes * area.height();
        assert!(output.len() >= length, "pixel output too short");
        if let Some(bytes) = self.contiguous(area, format) {
            output[..length].copy_from_slice(bytes);
            return length;
        }
        if let Storage::Native565(pixels) = self.storage {
            if format == PixelFormat::Rgb565Be {
                return packing::pack_rgb565(
                    pixels,
                    self.size,
                    (area.x0, area.y0, area.x1, area.y1),
                    area.height() as u16,
                    output,
                );
            }
        }
        for row in 0..area.height() {
            let y = area.y0 as usize + row;
            let target = &mut output[row * row_bytes..(row + 1) * row_bytes];
            if let Storage::Bytes(bytes, stored) = self.storage {
                if stored == format {
                    let start = y * self.stride + area.x0 as usize * format.bytes_per_pixel();
                    target.copy_from_slice(&bytes[start..start + row_bytes]);
                    continue;
                }
                if stored.bytes_per_pixel() == 2 && format.bytes_per_pixel() == 2 {
                    let start = y * self.stride + area.x0 as usize * 2;
                    for (dst, src) in target
                        .chunks_exact_mut(2)
                        .zip(bytes[start..start + row_bytes].chunks_exact(2))
                    {
                        dst.copy_from_slice(&[src[1], src[0]]);
                    }
                    continue;
                }
            }
            for (column, pixel) in target
                .chunks_exact_mut(format.bytes_per_pixel())
                .enumerate()
            {
                let (r, g, b) = self.rgb(area.x0 as usize + column, y);
                match format {
                    PixelFormat::Bgr888 => pixel.copy_from_slice(&[b, g, r]),
                    PixelFormat::Rgb565Le | PixelFormat::Rgb565Be => {
                        let value =
                            ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
                        pixel.copy_from_slice(&if format == PixelFormat::Rgb565Be {
                            value.to_be_bytes()
                        } else {
                            value.to_le_bytes()
                        });
                    }
                }
            }
        }
        length
    }

    fn rgb(self, x: usize, y: usize) -> (u8, u8, u8) {
        let value = match self.storage {
            Storage::Native565(pixels) => pixels[y * self.size.0 as usize + x],
            Storage::Bytes(bytes, format) => {
                let at = y * self.stride + x * format.bytes_per_pixel();
                match format {
                    PixelFormat::Bgr888 => return (bytes[at + 2], bytes[at + 1], bytes[at]),
                    PixelFormat::Rgb565Le => u16::from_le_bytes([bytes[at], bytes[at + 1]]),
                    PixelFormat::Rgb565Be => u16::from_be_bytes([bytes[at], bytes[at + 1]]),
                }
            }
        };
        let r = (value >> 11) as u8;
        let g = ((value >> 5) & 63) as u8;
        let b = (value & 31) as u8;
        (
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        )
    }

    pub fn rgb888_at(self, x: usize, y: usize) -> [u8; 3] {
        assert!(x < self.size.0 as usize && y < self.size.1 as usize);
        let (r, g, b) = self.rgb(x, y);
        [r, g, b]
    }

    /// Quantize to the existing RGB565 diagnostic capture protocol.
    pub fn rgb565_at(self, x: usize, y: usize) -> u16 {
        assert!(x < self.size.0 as usize && y < self.size.1 as usize);
        let (r, g, b) = self.rgb(x, y);
        ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
    }
}

/// One horizontal span of a composed row: `width` source columns from `src_x`.
#[derive(Clone, Copy)]
pub struct RowSegment<'a> {
    pub source: PixelSource<'a>,
    pub src_x: u16,
    pub width: u16,
}

/// Full rows composed left to right from up to two adjacent source spans.
///
/// A page push shows the end of one page beside the start of the other. Packing
/// both spans into each row lets the panel receive one window per row chunk in
/// top-to-bottom order, instead of two full-height column bands.
#[derive(Clone, Copy)]
pub struct SplitRows<'a> {
    segments: [Option<RowSegment<'a>>; 2],
    x0: u16,
    width: u16,
    height: u16,
}

impl<'a> SplitRows<'a> {
    /// `segments` are placed left to right starting at panel column `x0`.
    /// Zero-width segments are skipped. Rows `0..height` of every source are used.
    pub fn new(x0: u16, height: u16, segments: [Option<RowSegment<'a>>; 2]) -> Self {
        assert!(height > 0);
        let mut width = 0u16;
        for segment in segments.iter().flatten() {
            let (w, h) = segment.source.size();
            assert!(h >= height, "segment source shorter than the rows");
            assert!(
                u32::from(segment.src_x) + u32::from(segment.width) <= u32::from(w),
                "segment outside its source"
            );
            width = width
                .checked_add(segment.width)
                .expect("row width overflow");
        }
        assert!(width > 0, "composed rows need at least one pixel");
        Self {
            segments,
            x0,
            width,
            height,
        }
    }

    /// Panel window covering composed rows `y0..=y1`.
    pub fn window(&self, y0: u16, y1: u16) -> DirtyRect {
        DirtyRect {
            x0: self.x0,
            y0,
            x1: self.x0 + self.width - 1,
            y1,
        }
    }

    /// Panel window covering every composed row.
    pub fn area(&self) -> DirtyRect {
        self.window(0, self.height - 1)
    }

    /// Encode rows `y0..=y1` into `output`, row-major, returning the byte count.
    pub fn pack(&self, y0: u16, y1: u16, format: PixelFormat, output: &mut [u8]) -> usize {
        assert!(y0 <= y1 && y1 < self.height);
        let mut at = 0;
        for y in y0..=y1 {
            for segment in self.segments.iter().flatten() {
                if segment.width == 0 {
                    continue;
                }
                at += segment.source.pack(
                    DirtyRect {
                        x0: segment.src_x,
                        y0: y,
                        x1: segment.src_x + segment.width - 1,
                        y1: y,
                    },
                    format,
                    &mut output[at..],
                );
            }
        }
        at
    }
}

/// Split a rectangle into whole rows within the DMA payload and staging budget.
pub struct RowChunks {
    area: DirtyRect,
    next_y: u16,
    rows: u16,
}

impl RowChunks {
    pub fn new(area: DirtyRect, format: PixelFormat, capacity: usize) -> Self {
        assert!(area.is_valid(), "DMA rectangle must be inside the panel");
        let rows = capacity.min(MAX_DMA_BYTES) / (area.width() * format.bytes_per_pixel());
        assert!(rows > 0, "one pixel row must fit the transfer budget");
        Self {
            area,
            next_y: area.y0,
            rows: rows.min(u16::MAX as usize) as u16,
        }
    }
}

impl Iterator for RowChunks {
    type Item = DirtyRect;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next_y > self.area.y1 {
            return None;
        }
        let y0 = self.next_y;
        let y1 = (u32::from(y0) + u32::from(self.rows) - 1).min(u32::from(self.area.y1)) as u16;
        self.next_y = y1 + 1;
        Some(DirtyRect {
            y0,
            y1,
            ..self.area
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    fn rect(x0: u16, y0: u16, x1: u16, y1: u16) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn rgb565_primitives_expand_in_panel_bgr_order() {
        let source = PixelSource::rgb565(&[0xf800, 0x07e0, 0x001f, 0xffff], (4, 1));
        let mut bytes = [0; 12];
        source.pack(rect(0, 0, 3, 0), PixelFormat::Bgr888, &mut bytes);
        assert_eq!(bytes, [0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255]);
        for x in 0..4 {
            assert_eq!(source.rgb565_at(x, 0), [0xf800, 0x07e0, 0x001f, 0xffff][x]);
        }
    }

    #[test]
    fn partial_padded_bgr_rows_preserve_low_color_bits_and_exclude_padding() {
        let bytes = [1, 2, 3, 4, 5, 6, 99, 99, 7, 8, 9, 10, 11, 12];
        let source = PixelSource::bytes(&bytes, (2, 2), 8, PixelFormat::Bgr888);
        let mut out = [0xaa; 8];
        assert_eq!(
            source.pack(rect(1, 0, 1, 1), PixelFormat::Bgr888, &mut out),
            6
        );
        assert_eq!(out, [4, 5, 6, 10, 11, 12, 0xaa, 0xaa]);
        assert!(source
            .contiguous(rect(0, 0, 1, 1), PixelFormat::Bgr888)
            .is_none());
        assert_eq!(
            source.contiguous(rect(1, 1, 1, 1), PixelFormat::Bgr888),
            Some(&bytes[11..14])
        );
    }

    #[test]
    fn packed_rows_can_borrow_original_storage_and_565_byte_order_is_explicit() {
        let bytes = [0xf8, 0, 0, 0x1f, 0x07, 0xe0, 0xff, 0xff];
        let source = PixelSource::bytes(&bytes, (2, 2), 4, PixelFormat::Rgb565Be);
        assert_eq!(
            source
                .contiguous(rect(0, 1, 1, 1), PixelFormat::Rgb565Be)
                .unwrap()
                .as_ptr(),
            bytes[4..].as_ptr()
        );
        let mut out = [0; 8];
        source.pack(rect(0, 0, 1, 1), PixelFormat::Rgb565Le, &mut out);
        assert_eq!(out, [0, 0xf8, 0x1f, 0, 0xe0, 7, 0xff, 0xff]);
    }

    #[test]
    fn dma_chunks_cover_each_row_once_for_both_formats_and_final_short_chunk() {
        for format in [PixelFormat::Rgb565Be, PixelFormat::Bgr888] {
            let area = rect(0, 0, 479, 479);
            let chunks: Vec<_> = RowChunks::new(area, format, MAX_DMA_BYTES).collect();
            assert_eq!(
                chunks.len(),
                if format == PixelFormat::Bgr888 {
                    22
                } else {
                    15
                }
            );
            assert_eq!(chunks.iter().map(|c| c.height()).sum::<usize>(), 480);
            assert!(chunks
                .iter()
                .all(|c| c.width() * c.height() * format.bytes_per_pixel() <= MAX_DMA_BYTES));
            assert_eq!(chunks.last().unwrap().y1, 479);
        }
    }

    #[test]
    fn split_rows_place_both_spans_in_each_row_without_padding() {
        // Two 4x2 BGR sources with padded rows (stride 15 bytes).
        let left: Vec<u8> = (0..30).collect();
        let right: Vec<u8> = (100..130).collect();
        let left = PixelSource::bytes(&left, (4, 2), 15, PixelFormat::Bgr888);
        let right = PixelSource::bytes(&right, (4, 2), 15, PixelFormat::Bgr888);
        let rows = SplitRows::new(
            0,
            2,
            [
                Some(RowSegment {
                    source: left,
                    src_x: 2,
                    width: 2,
                }),
                Some(RowSegment {
                    source: right,
                    src_x: 0,
                    width: 2,
                }),
            ],
        );
        assert_eq!(rows.area(), rect(0, 0, 3, 1));
        let mut out = [0; 24];
        assert_eq!(rows.pack(0, 1, PixelFormat::Bgr888, &mut out), 24);
        assert_eq!(
            &out[..12],
            &[6, 7, 8, 9, 10, 11, 100, 101, 102, 103, 104, 105]
        );
        assert_eq!(
            &out[12..],
            &[21, 22, 23, 24, 25, 26, 115, 116, 117, 118, 119, 120]
        );
    }

    #[test]
    fn every_rgb565_value_survives_expansion_and_reencoding() {
        let values: Vec<u16> = (0..=u16::MAX).collect();
        let source = PixelSource::rgb565(&values, (256, 256));
        let mut bytes = vec![0; values.len() * 3];
        source.pack(rect(0, 0, 255, 255), PixelFormat::Bgr888, &mut bytes);
        let expanded = PixelSource::bytes(&bytes, (256, 256), 768, PixelFormat::Bgr888);
        for y in 0..256 {
            for x in 0..256 {
                assert_eq!(expanded.rgb565_at(x, y), values[y * 256 + x]);
            }
        }
    }
}
