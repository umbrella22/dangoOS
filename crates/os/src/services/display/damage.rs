//! Bounded display damage, preserving separated native flush regions.
use super::geometry::{DirtyRect, HEIGHT, WIDTH};

// Bound metadata and IRQ stack usage. Overflow coalesces to a covering rectangle.
const CAPACITY: usize = 16;
const EMPTY: DirtyRect = DirtyRect {
    x0: 1,
    y0: 1,
    x1: 0,
    y1: 0,
};
pub const FULL: DirtyRect = DirtyRect {
    x0: 0,
    y0: 0,
    x1: WIDTH as u16 - 1,
    y1: HEIGHT as u16 - 1,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Damage {
    regions: [DirtyRect; CAPACITY],
    len: usize,
}

impl Default for Damage {
    fn default() -> Self {
        Self::new()
    }
}

impl Damage {
    pub const fn new() -> Self {
        Self {
            regions: [EMPTY; CAPACITY],
            len: 0,
        }
    }
    pub fn full() -> Self {
        Self::from(FULL)
    }
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }
    pub fn regions(&self) -> &[DirtyRect] {
        &self.regions[..self.len]
    }
    pub fn pixels(&self) -> usize {
        self.regions().iter().map(|r| r.width() * r.height()).sum()
    }
    pub fn is_full(&self) -> bool {
        self.regions() == [FULL]
    }

    pub fn add(&mut self, mut rect: DirtyRect) {
        if rect.is_empty() {
            return;
        }
        assert!(rect.is_valid(), "damage outside display");
        // CO5300's four-column windows may include initialized unchanged shadow pixels.
        rect.x0 &= !3;
        rect.x1 = (rect.x1 | 3).min(WIDTH as u16 - 1);
        let mut i = 0;
        while i < self.len {
            let previous = self.regions[i];
            let joined = union(previous, rect);
            // Merge when this sends no more pixels than separate windows.
            if joined.width() * joined.height()
                <= previous.width() * previous.height() + rect.width() * rect.height()
            {
                rect = joined;
                self.len -= 1;
                self.regions[i] = self.regions[self.len];
                i = 0;
            } else {
                i += 1
            }
        }
        if self.len == CAPACITY {
            for old in self.regions() {
                rect = union(*old, rect)
            }
            self.len = 0;
        }
        self.regions[self.len] = rect;
        self.len += 1;
    }

    /// Include changes from a publication that never reached the panel.
    pub fn merge(&mut self, other: Self) {
        for rect in other.regions() {
            self.add(*rect)
        }
    }
}

impl From<DirtyRect> for Damage {
    fn from(rect: DirtyRect) -> Self {
        let mut damage = Self::new();
        damage.add(rect);
        damage
    }
}

fn union(a: DirtyRect, b: DirtyRect) -> DirtyRect {
    DirtyRect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(x: u16, y: u16) -> DirtyRect {
        DirtyRect {
            x0: x,
            y0: y,
            x1: x,
            y1: y,
        }
    }

    #[test]
    fn distant_controls_stay_local_and_edges_expand_to_panel_columns() {
        let mut d = Damage::from(point(1, 1));
        d.add(point(479, 479));
        assert_eq!(d.regions().len(), 2);
        assert_eq!(d.pixels(), 8);
        assert_eq!(
            d.regions()[0],
            DirtyRect {
                x0: 0,
                y0: 1,
                x1: 3,
                y1: 1
            }
        );
        assert_eq!(
            d.regions()[1],
            DirtyRect {
                x0: 476,
                y0: 479,
                x1: 479,
                y1: 479
            }
        );
    }

    #[test]
    fn adjacent_bands_merge_and_replaced_publications_retain_every_change() {
        let mut a = Damage::from(DirtyRect {
            x0: 0,
            y0: 0,
            x1: 479,
            y1: 23,
        });
        a.add(DirtyRect {
            x0: 0,
            y0: 24,
            x1: 479,
            y1: 47,
        });
        assert_eq!(
            a.regions(),
            &[DirtyRect {
                x0: 0,
                y0: 0,
                x1: 479,
                y1: 47
            }]
        );
        a.merge(Damage::from(point(479, 479)));
        assert_eq!(a.regions().len(), 2);
        a.merge(Damage::full());
        assert!(a.is_full());
    }

    #[test]
    fn metadata_overflow_still_covers_every_changed_pixel() {
        let mut d = Damage::new();
        for i in 0..50 {
            d.add(point(i * 8, i * 8))
        }
        for i in 0..50 {
            assert!(d
                .regions()
                .iter()
                .any(|r| r.x0 <= i * 8 && r.x1 >= i * 8 && r.y0 <= i * 8 && r.y1 >= i * 8));
        }
        assert!(d.regions().len() <= CAPACITY);
    }
}
