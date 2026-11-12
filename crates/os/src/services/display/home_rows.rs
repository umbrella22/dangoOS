//! A complete Home restore, encoded top to bottom without a full-screen copy.
use super::geometry::{DirtyRect, HEIGHT, WIDTH};
use crate::companion::{render, status};
use board_s31::pixels::{PixelFormat, PixelRows, PixelSource};

pub(crate) struct HomeRows<'a> {
    body: PixelSource<'a>,
    status: PixelSource<'a>,
    at: (u16, u16),
}

impl<'a> HomeRows<'a> {
    pub fn new(pixels: &'a [u16], at: (u16, u16)) -> Self {
        assert_eq!(pixels.len(), render::PIXELS + status::PIXELS);
        Self {
            body: PixelSource::rgb565(
                &pixels[..render::PIXELS],
                (render::W as u16, render::H as u16),
            ),
            status: PixelSource::rgb565(
                &pixels[render::PIXELS..],
                (status::WIDTH as u16, status::HEIGHT as u16),
            ),
            at,
        }
    }
}

impl PixelRows for HomeRows<'_> {
    fn area(&self) -> DirtyRect {
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: WIDTH as u16 - 1,
            y1: HEIGHT as u16 - 1,
        }
    }

    fn pack(&self, y0: u16, y1: u16, format: PixelFormat, output: &mut [u8]) -> usize {
        assert!(y0 <= y1 && y1 < HEIGHT as u16);
        let bpp = format.bytes_per_pixel();
        let stride = WIDTH * bpp;
        let length = (y1 - y0 + 1) as usize * stride;
        output[..length].fill(0);
        let status_at = (
            (i32::from(self.at.0) + status::OFFSET.0) as u16,
            (i32::from(self.at.1) + status::OFFSET.1) as u16,
        );
        for (source, at) in [(self.body, self.at), (self.status, status_at)] {
            let (w, h) = source.size();
            assert!(at.0 as usize + w as usize <= WIDTH && at.1 as usize + h as usize <= HEIGHT);
            let first = y0.max(at.1);
            let last = y1.min(at.1 + h - 1);
            for y in first..=last {
                let offset = (y - y0) as usize * stride + at.0 as usize * bpp;
                source.pack(
                    DirtyRect {
                        x0: 0,
                        y0: y - at.1,
                        x1: w - 1,
                        y1: y - at.1,
                    },
                    format,
                    &mut output[offset..offset + w as usize * bpp],
                );
            }
        }
        length
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::display::simulator::MemoryPanel;
    use alloc::{vec, vec::Vec};

    #[test]
    fn returning_home_overwrites_every_previous_page_pixel_in_both_formats() {
        let pixels: Vec<_> = (0..render::PIXELS + status::PIXELS)
            .map(|i| (i as u16).wrapping_mul(71) ^ 0x5aa5)
            .collect();
        let old = vec![0xffff; WIDTH * HEIGHT];
        for format in [PixelFormat::Rgb565Be, PixelFormat::Bgr888] {
            let mut panel = MemoryPanel::new((480, 480), format);
            let full = DirtyRect {
                x0: 0,
                y0: 0,
                x1: 479,
                y1: 479,
            };
            panel.upload(full, full, PixelSource::rgb565(&old, (480, 480)));
            let stats = panel.upload_rows(HomeRows::new(&pixels, (64, 88)));
            assert_eq!(stats.bytes, WIDTH * HEIGHT * format.bytes_per_pixel());
            assert_eq!(
                stats.windows,
                if format == PixelFormat::Bgr888 {
                    22
                } else {
                    15
                }
            );
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let expected = if (64..416).contains(&x) && (88..424).contains(&y) {
                        pixels[(y - 88) * render::W + x - 64]
                    } else if (200..280).contains(&x) && (40..74).contains(&y) {
                        pixels[render::PIXELS + (y - 40) * status::WIDTH + x - 200]
                    } else {
                        0
                    };
                    assert_eq!(
                        panel.pixels().rgb565_at(x, y),
                        expected,
                        "{format:?} ({x},{y})"
                    );
                }
            }
        }
    }
}
