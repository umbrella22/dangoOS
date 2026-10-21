//! Boot wordmark: the `ikaros` pixel type mark and its reveal timeline.
//!
//! The factory bootloader paints a `mosaico` splash before the application
//! runs; this module replaces that first impression as soon as the S31 app owns
//! the panel. The wordmark keeps the machine-made look of the official splash:
//! a bitmap font, one solid pixel per cell, no antialiasing.
//!
//! # Why a hand-written font
//!
//! The official pixel font ships inside the closed bootloader splash blob as a
//! raster image, not as a font file, so it cannot be reused directly. [`font`]
//! is a 5x7 dot-matrix face drawn to match that weight and pitch. When the
//! official glyph atlas is extracted (see `docs/ASSET_REQUESTS.md`), only
//! [`font`] needs to be replaced; the animation and the widget do not change.
//!
//! # Timeline
//!
//! ```text
//! letter 0      letter 1   ...  letter 5
//!   |--- 95 ms stagger ---|
//!                each letter: 240 ms top-down reveal
//!                             after the last letter: 260 ms underline sweep
//!                                                      380 ms hold
//!                                                      340 ms fade to black
//! ```
//!
//! Everything is a pure function of the elapsed time, so the host tests can
//! assert exact frames and the device renders the same pixels.

pub mod font;
pub mod widget;

pub use widget::{SplashControl, SplashWidget};

use font::{glyph, GLYPH_HEIGHT, GLYPH_WIDTH};

/// Round to the nearest integer without `f32::round` (`core` has no float
/// math on the target).
#[inline]
fn round_i32(value: f32) -> i32 {
    if value >= 0.0 {
        (value + 0.5) as i32
    } else {
        (value - 0.5) as i32
    }
}

/// One glyph advance in font cells: the glyph plus two blank columns.
///
/// Two cells instead of one gives the wordmark the airy tracking of the
/// factory splash; at 8 px per cell that is a 16 px gap between letters.
pub const GLYPH_ADVANCE: usize = GLYPH_WIDTH + 2;

/// Pixel scale used on the 480x480 panel.
pub const GLYPH_SCALE: usize = 8;

/// Splash canvas width: six letters at `GLYPH_ADVANCE`, plus margins.
pub const SPLASH_WIDTH: usize = 360;
/// Splash canvas height: wordmark plus the underline and margins.
pub const SPLASH_HEIGHT: usize = 120;
/// Splash canvas pixels.
pub const SPLASH_PIXELS: usize = SPLASH_WIDTH * SPLASH_HEIGHT;

/// The wordmark drawn on boot. Replacing this string is enough to rebrand.
pub const WORDMARK: &str = "ikaros";

/// Delay between the starts of two consecutive letters.
pub const LETTER_STAGGER_MS: u64 = 95;
/// Top-down reveal duration of one letter.
pub const LETTER_REVEAL_MS: u64 = 240;
/// Underline sweep duration (starts after the last letter is complete).
pub const UNDERLINE_MS: u64 = 260;
/// Full-word hold before the fade.
pub const HOLD_MS: u64 = 380;
/// Fade-to-black duration.
pub const FADE_MS: u64 = 340;

/// Wordmark body color: warm off-white.
const MARK_COLOR: (u8, u8, u8) = (0xf3, 0xf0, 0xea);
/// Underline accent: a dim warm gray so the line reads as a baseline, not text.
const RULE_COLOR: (u8, u8, u8) = (0x6b, 0x66, 0x5e);

/// Total splash duration in milliseconds.
pub const TOTAL_MS: u64 = {
    let letters = WORDMARK.len() as u64;
    (letters - 1) * LETTER_STAGGER_MS + LETTER_REVEAL_MS + UNDERLINE_MS + HOLD_MS + FADE_MS
};

/// Top-left origin of the wordmark inside the splash canvas.
pub fn wordmark_origin(width: usize) -> (usize, usize) {
    let word_width = WORDMARK.len() * GLYPH_ADVANCE * GLYPH_SCALE - GLYPH_SCALE;
    let x = width.saturating_sub(word_width) / 2;
    let text_height = GLYPH_HEIGHT * GLYPH_SCALE;
    let y = SPLASH_HEIGHT.saturating_sub(text_height) / 2;
    (x, y.saturating_sub(6))
}

/// Render one splash frame into `out` (`SPLASH_WIDTH * SPLASH_HEIGHT` RGB565).
///
/// Returns `true` once the timeline is complete; the last rendered frame is
/// fully black, so the caller can delete the splash canvas without a flash.
pub fn render(out: &mut [u16], elapsed_ms: u64) -> bool {
    debug_assert!(out.len() >= SPLASH_PIXELS);
    out.fill(0);

    let letters = WORDMARK.len() as u64;
    let reveal_start = elapsed_ms.min(u64::MAX);
    let underline_start = (letters - 1) * LETTER_STAGGER_MS + LETTER_REVEAL_MS;
    let fade_start = underline_start + UNDERLINE_MS + HOLD_MS;
    let alpha = if elapsed_ms <= fade_start {
        1.0
    } else if elapsed_ms >= fade_start + FADE_MS {
        0.0
    } else {
        1.0 - (elapsed_ms - fade_start) as f32 / FADE_MS as f32
    };

    let (origin_x, origin_y) = wordmark_origin(SPLASH_WIDTH);

    for (index, ch) in WORDMARK.chars().enumerate() {
        let start = index as u64 * LETTER_STAGGER_MS;
        if reveal_start < start {
            continue;
        }
        let progress = ((reveal_start - start) as f32 / LETTER_REVEAL_MS as f32).min(1.0);
        // The letter rises into place while it is revealed.
        let rise = round_i32((1.0 - progress) * 5.0);
        draw_glyph(
            out,
            ch,
            origin_x + index * GLYPH_ADVANCE * GLYPH_SCALE,
            origin_y + rise as usize,
            progress,
            alpha,
            MARK_COLOR,
        );
    }

    // Baseline rule: grows from the center outwards under the wordmark.
    if elapsed_ms >= underline_start {
        let progress = ((elapsed_ms - underline_start) as f32 / UNDERLINE_MS as f32).min(1.0);
        let word_width = WORDMARK.len() * GLYPH_ADVANCE * GLYPH_SCALE - GLYPH_SCALE;
        let full_width = (word_width as f32 * progress) as usize & !1;
        let center = SPLASH_WIDTH / 2;
        let x0 = center.saturating_sub(full_width / 2);
        let x1 = (x0 + full_width).min(SPLASH_WIDTH);
        let y = origin_y + GLYPH_HEIGHT * GLYPH_SCALE + 12;
        for yy in y..(y + 3) {
            for xx in x0..x1 {
                put(out, xx, yy, scale_color(RULE_COLOR, alpha));
            }
        }
    }

    elapsed_ms >= TOTAL_MS
}

/// Draw one character with a top-down reveal.
///
/// `progress` in `0..=1` limits the number of visible rows; `alpha` multiplies
/// the color for the fade-out.
fn draw_glyph(
    out: &mut [u16],
    ch: char,
    x: usize,
    y: usize,
    progress: f32,
    alpha: f32,
    color: (u8, u8, u8),
) {
    let rows = glyph(ch);
    let visible_rows = ((progress * GLYPH_HEIGHT as f32) as usize + 1).min(GLYPH_HEIGHT);
    let color = scale_color(color, alpha);

    for (row, bits) in rows.iter().enumerate().take(visible_rows.min(GLYPH_HEIGHT)) {
        for column in 0..GLYPH_WIDTH {
            if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                continue;
            }
            let px = x + column * GLYPH_SCALE;
            let py = y + row * GLYPH_SCALE;
            for dy in 0..GLYPH_SCALE {
                for dx in 0..GLYPH_SCALE {
                    put(out, px + dx, py + dy, color);
                }
            }
        }
    }
}

#[inline]
fn put(out: &mut [u16], x: usize, y: usize, color: u16) {
    if x < SPLASH_WIDTH && y < SPLASH_HEIGHT {
        out[y * SPLASH_WIDTH + x] = color;
    }
}

#[inline]
fn scale_color(rgb: (u8, u8, u8), alpha: f32) -> u16 {
    let a = alpha.clamp(0.0, 1.0);
    let r = (rgb.0 as f32 * a) as u8;
    let g = (rgb.1 as f32 * a) as u8;
    let b = (rgb.2 as f32 * a) as u8;
    let r5 = (r as u16 >> 3) & 0x1f;
    let g6 = (g as u16 >> 2) & 0x3f;
    let b5 = (b as u16 >> 3) & 0x1f;
    (r5 << 11) | (g6 << 5) | b5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timeline_is_ordered_and_finite() {
        assert_eq!(TOTAL_MS, 5 * LETTER_STAGGER_MS + LETTER_REVEAL_MS + UNDERLINE_MS + HOLD_MS + FADE_MS);
        assert!(TOTAL_MS > 1_000, "the splash is long enough to read");
        assert!(TOTAL_MS < 2_500, "the splash never delays a boot badly");
    }

    #[test]
    fn the_wordmark_is_centered_and_inside_the_canvas() {
        let (x, _) = wordmark_origin(SPLASH_WIDTH);
        let word_width = WORDMARK.len() * GLYPH_ADVANCE * GLYPH_SCALE;
        assert!(x + word_width <= SPLASH_WIDTH, "wordmark fits");
        assert!(
            SPLASH_WIDTH - (x + word_width) <= 2 * GLYPH_SCALE + 1,
            "the wordmark is centered"
        );
    }

    #[test]
    fn an_empty_frame_is_black_and_a_late_frame_fades_to_black() {
        let mut out = [0xffffu16; SPLASH_PIXELS];
        render(&mut out, 0);
        // At t=0 the first letter has just started, so nearly everything is
        // black; the corners must be black regardless.
        assert_eq!(out[0], 0);
        assert_eq!(out[SPLASH_PIXELS - 1], 0);

        let finished = render(&mut out, TOTAL_MS);
        assert!(finished);
        assert!(out.iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn every_letter_contributes_pixels() {
        let mut out = [0u16; SPLASH_PIXELS];
        render(&mut out, TOTAL_MS - HOLD_MS - FADE_MS + 10);
        // Slice the canvas into letter-sized columns and require each of the
        // first six columns to contain at least one lit pixel.
        let (origin_x, _) = wordmark_origin(SPLASH_WIDTH);
        for index in 0..WORDMARK.len() {
            let x0 = origin_x + index * GLYPH_ADVANCE * GLYPH_SCALE;
            let x1 = (x0 + GLYPH_WIDTH * GLYPH_SCALE).min(SPLASH_WIDTH);
            let lit = (0..SPLASH_HEIGHT).any(|y| (x0..x1).any(|x| out[y * SPLASH_WIDTH + x] != 0));
            assert!(lit, "letter {index} is visible");
        }
    }

    #[test]
    fn the_reveal_starts_with_the_first_letter_only() {
        let mut early = [0u16; SPLASH_PIXELS];
        render(&mut early, LETTER_REVEAL_MS + 5);
        let (origin_x, _) = wordmark_origin(SPLASH_WIDTH);
        let first = (origin_x..origin_x + GLYPH_WIDTH * GLYPH_SCALE)
            .any(|x| (0..SPLASH_HEIGHT).any(|y| early[y * SPLASH_WIDTH + x] != 0));
        let last_x = origin_x + (WORDMARK.len() - 1) * GLYPH_ADVANCE * GLYPH_SCALE;
        let last = (last_x..last_x + GLYPH_WIDTH * GLYPH_SCALE)
            .any(|x| (0..SPLASH_HEIGHT).any(|y| early[y * SPLASH_WIDTH + x] != 0));
        assert!(first, "the first letter is visible early");
        assert!(!last, "the last letter has not started yet");
    }

    #[test]
    fn the_underline_appears_after_the_letters() {
        let underline_start = (WORDMARK.len() as u64 - 1) * LETTER_STAGGER_MS + LETTER_REVEAL_MS;
        let rule_y = {
            let (_, y) = wordmark_origin(SPLASH_WIDTH);
            y + GLYPH_HEIGHT * GLYPH_SCALE + 12
        };
        let mut before = [0u16; SPLASH_PIXELS];
        render(&mut before, underline_start.saturating_sub(20));
        assert!(
            (0..SPLASH_WIDTH).all(|x| before[rule_y * SPLASH_WIDTH + x] == 0),
            "no rule before the sweep starts"
        );

        let mut after = [0u16; SPLASH_PIXELS];
        render(&mut after, underline_start + UNDERLINE_MS / 2);
        let lit = (0..SPLASH_WIDTH)
            .filter(|&x| after[rule_y * SPLASH_WIDTH + x] != 0)
            .count();
        assert!(lit > 20, "the rule has grown, got {lit} pixels");
        assert!(lit < SPLASH_WIDTH, "the rule has not finished yet");
    }
}
