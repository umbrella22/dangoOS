//! Page transitions between full-screen LVGL pages (launcher <-> app).
//!
//! LVGL renders every hub page into one 480x480 shadow frame, and the plane
//! task uploads rectangles of it to the panel. A slide therefore needs no
//! LVGL animation and no extra compose buffer: the UI task copies the old
//! page into a snapshot, renders the new page once, and each frame the plane
//! uploads two rectangles side by side, one from each buffer. This module is
//! the pure geometry of that slide; the device side lives in `tasks/ui.rs`,
//! `drivers/lvgl_port.rs` and `tasks/plane.rs`.
//!
//! - [`Direction::Forward`] (launcher -> app): the content moves left, the
//!   new page comes in from the right.
//! - [`Direction::Back`] (app -> launcher): the content moves right, the new
//!   page comes in from the left.
//!
//! Offsets are multiples of 4 because the panel takes 4-pixel aligned
//! columns (see `plane::align4`).

use crate::shell::navigation::route::Page;

/// Slide duration.
pub const SLIDE_MS: u64 = 280;
/// Panel column alignment.
const ALIGN: u16 = 4;

/// Which way the content moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Back,
}

impl Direction {
    /// The slide between two pages, if they get one. Only full-screen LVGL
    /// pages slide; the fox has its own entrance.
    pub fn between(from: Page, to: Page) -> Option<Direction> {
        match (from, to) {
            (Page::Launcher, Page::App(_)) => Some(Direction::Forward),
            (Page::App(_), Page::Launcher) => Some(Direction::Back),
            _ => None,
        }
    }
}

/// Which buffer a rectangle is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The snapshot of the page being left.
    Old,
    /// The shadow, already holding the page being entered.
    New,
}

/// One full-height column band: `width` columns read from `src_x` of
/// `source` are written at panel column `dst_x`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub source: Source,
    pub src_x: u16,
    pub dst_x: u16,
    pub width: u16,
}

/// Ease-out cubic: fast start, gentle landing.
fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// How far the new page has come in after `elapsed_ms`, in columns, 4-pixel
/// aligned, `0..=width`. Reaches `width` exactly at [`SLIDE_MS`].
pub fn offset(elapsed_ms: u64, width: u16) -> u16 {
    if elapsed_ms >= SLIDE_MS {
        return width;
    }
    let t = elapsed_ms as f32 / SLIDE_MS as f32;
    let raw = (ease_out(t) * width as f32) as u16;
    (raw / ALIGN * ALIGN).min(width)
}

/// The bands that tile the panel for one slide frame. Bands of zero width
/// are left out, so the result has one or two entries.
pub fn bands(direction: Direction, offset: u16, width: u16) -> ([Option<Band>; 2], usize) {
    let offset = offset.min(width);
    let rest = width - offset;
    let (first, second) = match direction {
        // [old shifted left | new entering from the right]
        Direction::Forward => (
            Band {
                source: Source::Old,
                src_x: offset,
                dst_x: 0,
                width: rest,
            },
            Band {
                source: Source::New,
                src_x: 0,
                dst_x: rest,
                width: offset,
            },
        ),
        // [new entering from the left | old shifted right]
        Direction::Back => (
            Band {
                source: Source::New,
                src_x: rest,
                dst_x: 0,
                width: offset,
            },
            Band {
                source: Source::Old,
                src_x: 0,
                dst_x: offset,
                width: rest,
            },
        ),
    };
    let mut out = [None, None];
    let mut count = 0;
    for band in [first, second] {
        if band.width > 0 {
            out[count] = Some(band);
            count += 1;
        }
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::navigation::route::AppId;

    const W: u16 = 480;

    #[test]
    fn only_launcher_and_app_pages_slide() {
        let app = Page::App(AppId::Settings);
        assert_eq!(
            Direction::between(Page::Launcher, app),
            Some(Direction::Forward)
        );
        assert_eq!(
            Direction::between(app, Page::Launcher),
            Some(Direction::Back)
        );
        assert_eq!(Direction::between(Page::Home, Page::Launcher), None);
        assert_eq!(Direction::between(Page::Launcher, Page::Home), None);
        assert_eq!(Direction::between(app, Page::Home), None);
    }

    #[test]
    fn the_offset_eases_out_aligned_and_lands_exactly() {
        assert_eq!(offset(0, W), 0);
        assert_eq!(offset(SLIDE_MS, W), W);
        assert_eq!(offset(SLIDE_MS * 3, W), W);
        let mut last = 0;
        for ms in 0..=SLIDE_MS {
            let o = offset(ms, W);
            assert_eq!(o % 4, 0, "{ms} ms");
            assert!(o >= last, "monotonic at {ms} ms");
            last = o;
        }
        // Ease-out: more than half way at the half time.
        assert!(offset(SLIDE_MS / 2, W) > W / 2 + W / 4);
    }

    #[test]
    fn the_bands_tile_the_panel_without_gaps() {
        for direction in [Direction::Forward, Direction::Back] {
            for off in (0..=W).step_by(4) {
                let (bands, count) = bands(direction, off, W);
                let mut covered = 0u16;
                let mut next_dst = 0u16;
                for band in bands.iter().take(count).flatten() {
                    assert_eq!(band.dst_x, next_dst);
                    assert!(band.src_x + band.width <= W);
                    assert_eq!(band.src_x % 4, 0);
                    assert_eq!(band.width % 4, 0);
                    next_dst += band.width;
                    covered += band.width;
                }
                assert_eq!(covered, W, "{direction:?} at {off}");
            }
        }
    }

    #[test]
    fn forward_shows_the_old_page_first_and_the_new_page_last() {
        let (b, n) = bands(Direction::Forward, 0, W);
        assert_eq!(n, 1);
        assert_eq!(b[0].unwrap().source, Source::Old);
        let (b, n) = bands(Direction::Forward, W, W);
        assert_eq!(n, 1);
        assert_eq!(
            b[0].unwrap(),
            Band {
                source: Source::New,
                src_x: 0,
                dst_x: 0,
                width: W
            }
        );
        // Mid-way the new page's left edge sits at the seam.
        let (b, _) = bands(Direction::Forward, 120, W);
        assert_eq!(
            b[1].unwrap(),
            Band {
                source: Source::New,
                src_x: 0,
                dst_x: 360,
                width: 120
            }
        );
    }

    #[test]
    fn back_brings_the_new_page_in_from_the_left() {
        let (b, _) = bands(Direction::Back, 120, W);
        assert_eq!(
            b[0].unwrap(),
            Band {
                source: Source::New,
                src_x: 360,
                dst_x: 0,
                width: 120
            }
        );
        assert_eq!(
            b[1].unwrap(),
            Band {
                source: Source::Old,
                src_x: 0,
                dst_x: 120,
                width: 360
            }
        );
    }
}
