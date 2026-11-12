//! The same static placeholder used before and between character loads.
//! Decodes directly into the existing frame lease without a heap allocation.
use crate::companion::render::{data, PIXELS};

pub fn render(out: &mut [u16]) {
    assert_eq!(out.len(), PIXELS);
    let mut at = 0;
    for pair in data::BLOB.chunks_exact(2) {
        let end = at + pair[0] as usize;
        out[at..end].fill(data::PALETTE[pair[1] as usize]);
        at = end;
    }
    assert_eq!(at, PIXELS);
}

pub fn render_panel(out: &mut [u16]) {
    use super::{VIEW_ORIGIN, VIEW_PIXELS, VIEW_W, W};
    assert_eq!(out.len(), VIEW_PIXELS);
    out.fill(0);
    let mut at = 0;
    for pair in data::BLOB.chunks_exact(2) {
        let start = (at / W + VIEW_ORIGIN.1) * VIEW_W + at % W + VIEW_ORIGIN.0;
        out[start..start + pair[0] as usize].fill(data::PALETTE[pair[1] as usize]);
        at += pair[0] as usize;
    }
    assert_eq!(at, PIXELS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::companion::render::{FoxPose, FoxRenderer, Scene, W};
    #[test]
    fn loading_uses_the_builtin_image_and_every_run_stays_within_one_row() {
        let mut pixels = alloc::vec![0xffff; PIXELS];
        render(&mut pixels);
        let mut expected = alloc::vec![0; PIXELS];
        FoxRenderer::new().render(&Scene::still(FoxPose::Idle), &mut expected);
        assert_eq!(pixels, expected);
        let mut width = 0;
        for pair in data::BLOB.chunks_exact(2) {
            assert!(pair[0] > 0);
            assert!((pair[1] as usize) < data::PALETTE.len());
            width += pair[0] as usize;
            assert!(width <= W);
            if width == W {
                width = 0;
            }
        }
        assert_eq!(width, 0);
    }
}
