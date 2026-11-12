//! Host memory panel: production pixel encoders/chunk plans with immediate CPU completion.
//! Native LVGL runs normally. This backend models neither DMA timing nor optical scanning.
use alloc::{vec, vec::Vec};
use board_s31::{
    geometry::DirtyRect,
    pixels::{PixelFormat, PixelSource, RowChunks, SplitRows, MAX_DMA_BYTES},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Upload {
    pub bytes: usize,
    pub direct_bytes: usize,
    pub windows: usize,
}

pub struct MemoryPanel {
    pixels: Vec<u8>,
    size: (u16, u16),
    format: PixelFormat,
}

impl MemoryPanel {
    pub fn new(size: (u16, u16), format: PixelFormat) -> Self {
        Self {
            pixels: vec![0; size.0 as usize * size.1 as usize * format.bytes_per_pixel()],
            size,
            format,
        }
    }

    pub fn pixels(&self) -> PixelSource<'_> {
        PixelSource::bytes(
            &self.pixels,
            self.size,
            self.size.0 as usize * self.format.bytes_per_pixel(),
            self.format,
        )
    }

    /// Compare the incrementally updated panel to the complete source frame.
    pub fn matches(&self, source: PixelSource<'_>) -> bool {
        let mut expected = vec![0; self.pixels.len()];
        source.pack(
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: self.size.0 - 1,
                y1: self.size.1 - 1,
            },
            self.format,
            &mut expected,
        );
        self.pixels == expected
    }

    /// Apply the same whole-row chunks and encoded bytes as the device driver.
    pub fn upload(&mut self, dst: DirtyRect, src: DirtyRect, source: PixelSource<'_>) -> Upload {
        assert!(dst.x1 < self.size.0 && dst.y1 < self.size.1);
        assert_eq!((dst.width(), dst.height()), (src.width(), src.height()));
        let mut staging = vec![0; MAX_DMA_BYTES];
        let mut stats = Upload::default();
        for part in RowChunks::new(src, self.format, MAX_DMA_BYTES) {
            let direct = source.contiguous(part, self.format);
            let bytes = if let Some(bytes) = direct {
                stats.direct_bytes += bytes.len();
                bytes
            } else {
                let length = source.pack(part, self.format, &mut staging);
                &staging[..length]
            };
            let row_bytes = part.width() * self.format.bytes_per_pixel();
            for row in 0..part.height() {
                let y = dst.y0 as usize + part.y0 as usize - src.y0 as usize + row;
                let offset =
                    (y * self.size.0 as usize + dst.x0 as usize) * self.format.bytes_per_pixel();
                self.pixels[offset..offset + row_bytes]
                    .copy_from_slice(&bytes[row * row_bytes..(row + 1) * row_bytes]);
            }
            stats.bytes += bytes.len();
            stats.windows += 1;
        }
        stats
    }

    /// Apply composed full rows (a page push) with the driver's row-major chunks.
    pub fn upload_rows(&mut self, rows: SplitRows<'_>) -> Upload {
        let area = rows.area();
        assert!(area.x1 < self.size.0 && area.y1 < self.size.1);
        let mut staging = vec![0; MAX_DMA_BYTES];
        let mut stats = Upload::default();
        for part in RowChunks::new(area, self.format, MAX_DMA_BYTES) {
            let length = rows.pack(part.y0, part.y1, self.format, &mut staging);
            let row_bytes = part.width() * self.format.bytes_per_pixel();
            for row in 0..part.height() {
                let y = part.y0 as usize + row;
                let offset =
                    (y * self.size.0 as usize + part.x0 as usize) * self.format.bytes_per_pixel();
                self.pixels[offset..offset + row_bytes]
                    .copy_from_slice(&staging[row * row_bytes..(row + 1) * row_bytes]);
            }
            stats.bytes += length;
            stats.windows += 1;
        }
        stats
    }

    pub fn ppm(&self) -> Vec<u8> {
        let mut out = alloc::format!("P6\n{} {}\n255\n", self.size.0, self.size.1).into_bytes();
        let source = self.pixels();
        for y in 0..self.size.1 as usize {
            for x in 0..self.size.0 as usize {
                out.extend_from_slice(&source.rgb888_at(x, y));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::navigation::transition::{self, Direction, Source};

    #[test]
    fn partial_window_preserves_surrounding_pixels_and_padded_source_rows() {
        let mut panel = MemoryPanel::new((8, 4), PixelFormat::Bgr888);
        let mut bytes = [0xee; 32 * 4];
        for y in 0..4 {
            for x in 0..8 {
                bytes[y * 32 + x * 3..y * 32 + x * 3 + 3]
                    .copy_from_slice(&[x as u8, y as u8, 0x13]);
            }
        }
        let source = PixelSource::bytes(&bytes, (8, 4), 32, PixelFormat::Bgr888);
        let dst = DirtyRect {
            x0: 0,
            y0: 1,
            x1: 3,
            y1: 2,
        };
        let src = DirtyRect {
            x0: 4,
            y0: 2,
            x1: 7,
            y1: 3,
        };
        let stats = panel.upload(dst, src, source);
        assert_eq!((stats.bytes, stats.direct_bytes, stats.windows), (24, 0, 1));
        assert_eq!(panel.pixels().rgb888_at(0, 1), [0x13, 2, 4]);
        assert_eq!(panel.pixels().rgb888_at(3, 2), [0x13, 3, 7]);
        assert_eq!(panel.pixels().rgb888_at(4, 1), [0, 0, 0]);
    }

    #[test]
    fn push_rows_compose_both_pages_full_width_without_a_compose_frame() {
        // Column-coded pages so a misplaced band is visible at every pixel.
        let page = |tag: u8| {
            let mut bytes = vec![0; 480 * 480 * 3];
            for (i, px) in bytes.chunks_exact_mut(3).enumerate() {
                let x = (i % 480) as u16;
                px.copy_from_slice(&[tag, x as u8, (x >> 8) as u8]);
            }
            bytes
        };
        let (old_bytes, new_bytes) = (page(7), page(19));
        let old = PixelSource::bytes(&old_bytes, (480, 480), 1440, PixelFormat::Bgr888);
        let new = PixelSource::bytes(&new_bytes, (480, 480), 1440, PixelFormat::Bgr888);
        let mut panel = MemoryPanel::new((480, 480), PixelFormat::Bgr888);
        for direction in [Direction::Forward, Direction::Back] {
            for offset in [0, 4, 128, 476, 480] {
                let stats = panel.upload_rows(transition::rows(new, old, direction, offset));
                assert_eq!(stats.bytes, 480 * 480 * 3);
                // Whole-width chunks only: no per-band windows.
                assert_eq!(stats.windows, 22);
                let (bands, count) = transition::bands(direction, offset, 480);
                for band in bands.iter().take(count).flatten() {
                    let (source, tag) = match band.source {
                        Source::New => (new, 19),
                        Source::Old => (old, 7),
                    };
                    for x in [band.dst_x, band.dst_x + band.width - 1] {
                        let src_x = (band.src_x + x - band.dst_x) as usize;
                        for y in [0, 239, 479] {
                            assert_eq!(
                                panel.pixels().rgb888_at(x as usize, y),
                                source.rgb888_at(src_x, y)
                            );
                            assert_eq!(panel.pixels().rgb888_at(x as usize, y)[2], tag);
                        }
                    }
                }
            }
        }
    }
}
