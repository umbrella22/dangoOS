//! Pure RGB565 window packing; can be host-tested with `rustc --test`.

/// Pack an inclusive window from a full row-major framebuffer into big-endian
/// bytes. All programmer preconditions are checked before writing staging.
/// No pixel alignment policy is imposed by this transport helper.
// The archived upstream helper is kept verbatim; `as_chunks_mut` is a style
// suggestion only.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub(crate) fn pack_rgb565(
    framebuffer: &[u16],
    panel_size: (u16, u16),
    window: (u16, u16, u16, u16),
    max_rows: u16,
    staging: &mut [u8],
) -> usize {
    let (panel_width, panel_height) = panel_size;
    let (x1, y1, x2, y2) = window;
    assert!(x1 <= x2 && y1 <= y2, "inverted chunk window");
    assert!(x2 < panel_width && y2 < panel_height, "chunk outside panel");

    let stride = panel_width as usize;
    let width = (x2 - x1) as usize + 1;
    let rows = (y2 - y1) as usize + 1;
    assert!(rows <= max_rows as usize, "chunk exceeds row limit");
    assert!(
        framebuffer.len() >= stride * panel_height as usize,
        "full panel framebuffer required",
    );
    let byte_count = width * rows * 2;
    assert!(staging.len() >= byte_count, "staging too small");

    for (row_index, dst_row) in staging[..byte_count]
        .chunks_exact_mut(width * 2)
        .enumerate()
    {
        let start = (y1 as usize + row_index) * stride + x1 as usize;
        let src = &framebuffer[start..start + width];

        // Two pixels per 32-bit store: byte swapping is cheap, but halving
        // the loop count and the per-iteration bookkeeping is not. Damage
        // rectangles are 4-pixel aligned in x, so a real chunk is even; the
        // remainder path only handles an odd final column.
        let mut pairs = src.chunks_exact(2);
        for (dst, pair) in dst_row.chunks_exact_mut(4).zip(&mut pairs) {
            let word = ((pair[0] as u32) << 16) | pair[1] as u32;
            dst.copy_from_slice(&word.to_be_bytes());
        }
        if let Some(&last) = pairs.remainder().first() {
            let tail = &mut dst_row[width * 2 - 2..];
            tail.copy_from_slice(&last.to_be_bytes());
        }
    }
    byte_count
}

#[cfg(test)]
mod tests {
    use super::pack_rgb565;

    const PANEL: (u16, u16) = (480, 480);
    const MAX_ROWS: u16 = 16;
    const CAPACITY: usize = MAX_ROWS as usize * PANEL.0 as usize * 2;

    #[test]
    fn primary_colors_are_big_endian() {
        let mut dst = [0; 8];
        let bytes = pack_rgb565(
            &[0xF800, 0x07E0, 0x001F, 0x1234],
            (4, 1),
            (0, 0, 3, 0),
            MAX_ROWS,
            &mut dst,
        );
        assert_eq!(bytes, 8);
        assert_eq!(dst, [0xF8, 0, 0x07, 0xE0, 0, 0x1F, 0x12, 0x34]);
    }

    #[test]
    fn partial_window_uses_full_framebuffer_stride_without_alignment() {
        let fb: Vec<u16> = (0..20).collect();
        let mut dst = [0xAA; 16];
        let bytes = pack_rgb565(&fb, (5, 4), (1, 1, 3, 2), MAX_ROWS, &mut dst);
        assert_eq!(bytes, 12);
        assert_eq!(&dst[..bytes], &[0, 6, 0, 7, 0, 8, 0, 11, 0, 12, 0, 13]);
        assert_eq!(&dst[bytes..], &[0xAA; 4]);
    }

    #[test]
    fn single_bottom_right_pixel_is_inclusive() {
        let mut fb = vec![0; PANEL.0 as usize * PANEL.1 as usize];
        *fb.last_mut().unwrap() = 0xABCD;
        let mut dst = [0xAA; 4];
        assert_eq!(
            pack_rgb565(&fb, PANEL, (479, 479, 479, 479), MAX_ROWS, &mut dst),
            2
        );
        assert_eq!(dst, [0xAB, 0xCD, 0xAA, 0xAA]);
    }

    #[test]
    fn full_width_sixteen_row_chunk_fits_exact_capacity() {
        let fb: Vec<u16> = (0..PANEL.0 as usize * PANEL.1 as usize)
            .map(|i| i as u16)
            .collect();
        let mut dst = vec![0; CAPACITY];
        assert_eq!(
            pack_rgb565(&fb, PANEL, (0, 464, 479, 479), MAX_ROWS, &mut dst),
            CAPACITY
        );
        for (pixel, bytes) in fb[464 * 480..].iter().zip(dst.chunks_exact(2)) {
            assert_eq!(bytes, pixel.to_be_bytes());
        }
    }

    #[test]
    fn short_final_chunk_does_not_send_stale_tail() {
        let fb = vec![0x1357; PANEL.0 as usize * PANEL.1 as usize];
        let mut dst = vec![0xAA; CAPACITY];
        let bytes = pack_rgb565(&fb, PANEL, (0, 477, 479, 479), MAX_ROWS, &mut dst);
        assert_eq!(bytes, 3 * 480 * 2);
        assert!(dst[..bytes].chunks_exact(2).all(|p| p == [0x13, 0x57]));
        assert!(dst[bytes..].iter().all(|b| *b == 0xAA));
    }

    fn assert_invalid(window: (u16, u16, u16, u16), fb_len: usize, dst_len: usize) {
        let fb = vec![0; fb_len];
        let mut dst = vec![0xAA; dst_len];
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pack_rgb565(&fb, PANEL, window, MAX_ROWS, &mut dst)
        }));
        assert!(result.is_err(), "invalid precondition accepted: {window:?}");
        assert!(
            dst.iter().all(|b| *b == 0xAA),
            "staging modified before validation"
        );
    }

    #[test]
    fn inverted_windows_are_rejected_before_packing() {
        assert_invalid((2, 0, 1, 0), 480 * 480, CAPACITY);
        assert_invalid((0, 2, 0, 1), 480 * 480, CAPACITY);
    }

    #[test]
    fn out_of_bounds_windows_are_rejected_before_packing() {
        for window in [
            (0, 0, 480, 0),
            (0, 0, 0, 480),
            (480, 0, 480, 0),
            (0, 480, 0, 480),
            (0, 0, u16::MAX, u16::MAX),
        ] {
            assert_invalid(window, 480 * 480, CAPACITY);
        }
    }

    #[test]
    fn seventeen_rows_are_rejected_even_for_one_pixel_width() {
        assert_invalid((1, 0, 1, 16), 480 * 480, CAPACITY);
    }

    #[test]
    fn short_framebuffer_is_rejected_even_for_top_left_pixel() {
        assert_invalid((0, 0, 0, 0), 480 * 480 - 1, CAPACITY);
    }

    #[test]
    fn short_staging_is_rejected_before_packing() {
        assert_invalid((0, 0, 479, 15), 480 * 480, CAPACITY - 1);
    }
}
