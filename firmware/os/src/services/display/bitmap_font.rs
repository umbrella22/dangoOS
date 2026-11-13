//! Fixed-width 5x7 dot-matrix face for the boot wordmark and canvas status text.
//!
//! Each glyph is seven rows of five bits; bit 4 is the leftmost column. The
//! design follows the classic terminal/dot-matrix proportion (1 px stems, 1 px
//! spacing) so the renderer can scale it to solid 8x8 blocks without changing
//! the silhouette.
//!

/// Glyph width in font cells.
pub const GLYPH_WIDTH: usize = 5;
/// Glyph height in font cells.
pub const GLYPH_HEIGHT: usize = 7;

/// Blank glyph rows (used for space and unknown characters).
const BLANK: [u8; GLYPH_HEIGHT] = [0, 0, 0, 0, 0, 0, 0];

/// Return the seven row bitmaps for `ch`.
///
/// Lowercase letters, digits and a few separators are defined; anything else
/// renders as a blank advance, which is the safe behavior for an unknown
/// wordmark string.
pub fn glyph(ch: char) -> &'static [u8; GLYPH_HEIGHT] {
    match ch {
        'a' => &[
            0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10001, 0b01110,
        ],
        'b' => &[
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110,
        ],
        'c' => &[
            0b00000, 0b00000, 0b01110, 0b10001, 0b10000, 0b10001, 0b01110,
        ],
        'd' => &[
            0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111,
        ],
        'e' => &[
            0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110,
        ],
        'f' => &[
            0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000,
        ],
        'g' => &[
            0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
        ],
        'h' => &[
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'i' => &[
            0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'j' => &[
            0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'k' => &[
            0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010,
        ],
        'l' => &[
            0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'm' => &[
            0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001,
        ],
        'n' => &[
            0b00000, 0b00000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'o' => &[
            0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'p' => &[
            0b00000, 0b00000, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000,
        ],
        'q' => &[
            0b00000, 0b00000, 0b01101, 0b10011, 0b01111, 0b00001, 0b00001,
        ],
        'r' => &[
            0b00000, 0b00000, 0b10111, 0b11001, 0b10000, 0b10000, 0b10000,
        ],
        's' => &[
            0b00000, 0b00000, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110,
        ],
        't' => &[
            0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110,
        ],
        'u' => &[
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101,
        ],
        'v' => &[
            0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'w' => &[
            0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010,
        ],
        'x' => &[
            0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001,
        ],
        'y' => &[
            0b00000, 0b10001, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110,
        ],
        'z' => &[
            0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '0' => &[
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => &[
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => &[
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => &[
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => &[
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => &[
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => &[
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => &[
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => &[
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => &[
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        '-' => &[
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '.' => &[
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        ':' => &[
            0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000,
        ],
        '!' => &[
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100,
        ],
        '?' => &[
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100,
        ],
        _ => &BLANK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wordmark_letter_has_ink() {
        for ch in "ikaros".chars() {
            let rows = glyph(ch);
            let lit: u32 = rows.iter().map(|row| row.count_ones()).sum();
            assert!(lit >= 5, "glyph {ch:?} has enough ink ({lit})");
        }
    }

    #[test]
    fn unknown_characters_are_blank() {
        assert_eq!(glyph('\u{1F600}').iter().sum::<u8>(), 0);
        assert_eq!(glyph(' '), &BLANK);
    }

    #[test]
    fn glyphs_stay_inside_five_columns() {
        for ch in "abcdefghijklmnopqrstuvwxyz0123456789-.:!?".chars() {
            for row in glyph(ch) {
                assert_eq!(row & !0b11111, 0, "glyph {ch:?} row overflows");
            }
        }
    }
}
