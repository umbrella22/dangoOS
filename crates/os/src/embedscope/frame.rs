//! Owned final-output pixels, independent of LVGL and DMA leases.
use crate::services::display::geometry::DirtyRect;
use alloc::{boxed::Box, vec};

pub(super) struct FinalFrame {
    pub pixels: Box<[u16]>,
    width: usize,
    height: usize,
    pub id: u64,
}
impl FinalFrame {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![0; width * height].into_boxed_slice(),
            width,
            height,
            id: 0,
        }
    }
    pub fn clear_outside(&mut self, at: (u16, u16), size: (u16, u16)) {
        for y in 0..self.height {
            for x in 0..self.width {
                if x < at.0 as usize
                    || x >= (at.0 + size.0) as usize
                    || y < at.1 as usize
                    || y >= (at.1 + size.1) as usize
                {
                    self.pixels[y * self.width + x] = 0;
                }
            }
        }
    }
    pub fn blit(&mut self, source: &[u16], pitch: usize, area: DirtyRect, at: (u16, u16)) {
        if area.is_empty() {
            return;
        }
        let w = area.width();
        for row in 0..area.height() {
            let from = (area.y0 as usize + row) * pitch + area.x0 as usize;
            let to = (at.1 as usize + row) * self.width + at.0 as usize;
            debug_assert!(at.0 as usize + w <= self.width && at.1 as usize + row < self.height);
            self.pixels[to..to + w].copy_from_slice(&source[from..from + w]);
        }
    }
    /// The diagnostic protocol currently exposes RGB565, including RGB888 panels.
    pub fn blit_source(
        &mut self,
        source: board_s31::pixels::PixelSource<'_>,
        area: DirtyRect,
        at: (u16, u16),
    ) {
        for row in 0..area.height() {
            for column in 0..area.width() {
                self.pixels[(at.1 as usize + row) * self.width + at.0 as usize + column] =
                    source.rgb565_at(area.x0 as usize + column, area.y0 as usize + row);
            }
        }
    }
    pub fn complete(&mut self) {
        self.id = self.id.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(w: u16, h: u16) -> DirtyRect {
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: w - 1,
            y1: h - 1,
        }
    }
    #[test]
    fn final_output_preserves_damage_status_and_frozen_pixels() {
        let mut f = FinalFrame::new(8, 8);
        f.blit(&[7; 64], 8, rect(8, 8), (0, 0));
        f.clear_outside((2, 3), (4, 4));
        f.blit(&[9; 16], 4, rect(4, 4), (2, 3));
        f.blit(&[5; 4], 2, rect(2, 2), (3, 0));
        f.complete();
        let frozen = f.pixels.clone();
        f.blit(&[3], 1, rect(1, 1), (2, 3));
        f.complete();
        assert_eq!(frozen[3 * 8 + 2], 9);
        assert_eq!(f.pixels[3 * 8 + 2], 3);
        assert_eq!(frozen[3], 5);
        assert_eq!(frozen[7], 0);
        assert_eq!(f.id, 2);
    }
    #[test]
    fn slide_bands_compose_old_and_new_columns() {
        let mut f = FinalFrame::new(8, 4);
        f.blit(
            &[1; 32],
            8,
            DirtyRect {
                x0: 4,
                y0: 0,
                x1: 7,
                y1: 3,
            },
            (0, 0),
        );
        f.blit(
            &[2; 32],
            8,
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: 3,
                y1: 3,
            },
            (4, 0),
        );
        assert!(f.pixels.chunks(8).all(|r| r == [1, 1, 1, 1, 2, 2, 2, 2]));
    }
}
