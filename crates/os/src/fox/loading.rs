//! Skin-independent loading art. Pure RGB565 drawing; no filesystem or decoder.
use super::{H, W};

/// A quiet dango silhouette and a central question mark, on the Home background.
pub fn render(out: &mut [u16]) {
    assert_eq!(out.len(), W * H);
    out.fill(0);
    for y in 0..H {
        for x in 0..W {
            let dx = x as i32 - W as i32 / 2;
            let dy = y as i32 - H as i32 / 2 - 6;
            // Normalized ellipse, with a soft rim rather than a facial feature.
            let radius = dx * dx * 1024 / (112 * 112) + dy * dy * 1024 / (92 * 92);
            let gray = if radius < 960 {
                36 + (92 - dy).clamp(0, 184) / 7
            } else if radius < 1120 {
                (1120 - radius) * 48 / 160
            } else {
                0
            } as u16;
            out[y * W + x] = ((gray >> 3) << 11) | ((gray >> 2) << 5) | (gray >> 3);
        }
    }
    // Pixel glyph deliberately independent of the selected character/fonts.
    let question = [0b01110, 0b11011, 0b00011, 0b00110, 0b00100, 0, 0b00100];
    let scale = 7;
    let x0 = (W - 5 * scale) / 2;
    let y0 = (H - 7 * scale) / 2 + 6;
    for (row, bits) in question.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                for y in 0..scale {
                    for x in 0..scale {
                        out[(y0 + row * scale + y) * W + x0 + col * scale + x] = 0xce79;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silhouette_has_no_skin_colors_and_the_question_is_centered() {
        let mut pixels = alloc::vec![0xffff; W * H];
        render(&mut pixels);
        assert_eq!(pixels[0], 0);
        assert_eq!(pixels[H * W - 1], 0);
        assert_eq!(pixels[(H / 2 + 6) * W + W / 2], 0xce79);
        assert!(pixels.iter().any(|&p| p != 0 && p != 0xce79));
        for &p in &pixels {
            let r = (p >> 11) & 31;
            let g = (p >> 5) & 63;
            assert_eq!(r, p & 31);
            assert!((i32::from(g) - i32::from(r * 2)).abs() <= 1);
        }
    }
}
