//! Panel geometry: the 480x480 CO5300 and its inclusive window type.
//!
//! Pure and dependency-free so `scripts/check/drivers.sh` can host-test it with
//! `rustc --test` (the firmware bin itself is built with `test = false`).

pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 480;

/// An inclusive, panel-relative window, matching the CO5300 `CASET`/`RASET`
/// commands. A rectangle with `x0 > x1` or `y0 > y1` is empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}

impl DirtyRect {
    /// True when the rectangle covers no pixel.
    pub const fn is_empty(self) -> bool {
        self.x0 > self.x1 || self.y0 > self.y1
    }

    /// True for a non-empty rectangle fully inside the panel.
    pub const fn is_valid(self) -> bool {
        !self.is_empty() && (self.x1 as usize) < WIDTH && (self.y1 as usize) < HEIGHT
    }

    /// Inclusive width; zero for an empty rectangle.
    pub const fn width(self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.x1 as usize + 1 - self.x0 as usize
        }
    }

    /// Inclusive height; zero for an empty rectangle.
    pub const fn height(self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.y1 as usize + 1 - self.y0 as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rect(x0: u16, y0: u16, x1: u16, y1: u16) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn sizes_are_inclusive() {
        let r = rect(4, 10, 7, 10);
        assert_eq!((r.width(), r.height()), (4, 1));
        assert!(r.is_valid());
    }

    #[test]
    fn inverted_rectangles_are_empty_and_invalid() {
        let r = rect(5, 0, 4, 0);
        assert!(r.is_empty());
        assert!(!r.is_valid());
        assert_eq!((r.width(), r.height()), (0, 0));
    }

    #[test]
    fn rectangles_past_the_panel_are_invalid() {
        assert!(rect(0, 0, 479, 479).is_valid());
        assert!(!rect(0, 0, 480, 0).is_valid());
        assert!(!rect(0, 0, 0, 480).is_valid());
    }
}
