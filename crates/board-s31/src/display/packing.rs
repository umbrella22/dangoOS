//! Pure RGB565 window packing; can be host-tested with `rustc --test`.

/// Pack an inclusive window from a full row-major framebuffer into big-endian
/// bytes. All programmer preconditions are checked before writing staging.
/// No pixel alignment policy is imposed by this transport helper.
// `as_chunks_mut` is a style suggestion; the explicit pair loop also supports
// byte-aligned output with an odd final column.
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

        if dst_row.as_ptr().align_offset(core::mem::align_of::<u32>()) == 0 {
            // A byte slice only promises byte alignment. Without this checked
            // word path LLVM emits four PSRAM byte stores per pixel pair on
            // RV32, even for copy_from_slice(&word.to_be_bytes()).
            for (index, pair) in src.chunks_exact(2).enumerate() {
                let a = pair[0].to_be_bytes();
                let b = pair[1].to_be_bytes();
                let word = u32::from_ne_bytes([a[0], a[1], b[0], b[1]]);
                // SAFETY: the row starts word-aligned, each index advances
                // four bytes, and each complete pair owns four output bytes.
                unsafe { dst_row.as_mut_ptr().cast::<u32>().add(index).write(word) };
            }
            if width % 2 != 0 {
                dst_row[width * 2 - 2..].copy_from_slice(&src[width - 1].to_be_bytes());
            }
            continue;
        }

        // A byte-aligned fallback keeps arbitrary callers valid. Real panel
        // windows are four-column aligned and normally use the word path.
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

/// Reverse the two bytes of each RGB565 pixel without changing pixel order.
/// Both byte orders use this operation; arbitrary input/output alignment is
/// supported, while aligned panel rows use word loads and stores on RV32.
pub(crate) fn swap_rgb565_bytes(input: &[u8], output: &mut [u8]) {
    assert_eq!(input.len() % 2, 0, "RGB565 needs complete pixels");
    assert!(output.len() >= input.len(), "RGB565 output too short");
    let mut at = 0;
    if input.as_ptr().align_offset(4) == 0 && output.as_ptr().align_offset(4) == 0 {
        while at + 4 <= input.len() {
            // SAFETY: both bases are word-aligned, at is a multiple of four,
            // and the loop bounds leave a complete initialized word in each.
            unsafe {
                let word = input.as_ptr().add(at).cast::<u32>().read();
                let swapped = ((word & 0x00ff_00ff) << 8) | ((word & 0xff00_ff00) >> 8);
                output.as_mut_ptr().add(at).cast::<u32>().write(swapped);
            }
            at += 4;
        }
    }
    for (dst, src) in output[at..input.len()]
        .chunks_exact_mut(2)
        .zip(input[at..].chunks_exact(2))
    {
        dst.copy_from_slice(&[src[1], src[0]]);
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::pack_rgb565;
    use alloc::{vec, vec::Vec};

    const PANEL: (u16, u16) = (480, 480);
    const MAX_ROWS: u16 = 16;
    const CAPACITY: usize = MAX_ROWS as usize * PANEL.0 as usize * 2;

    #[test]
    fn byte_swap_preserves_all_colors_at_every_alignment_and_odd_pixel_tail() {
        let mut values: Vec<u16> = (0..=u16::MAX).collect();
        values.push(0xABCD);
        for src_offset in 0..4 {
            for dst_offset in 0..4 {
                let pixels = &values[..];
                let mut input = vec![0x33; src_offset];
                input.extend(pixels.iter().flat_map(|p| p.to_le_bytes()));
                let mut output = vec![0x55; dst_offset + pixels.len() * 2 + 4];
                super::swap_rgb565_bytes(&input[src_offset..], &mut output[dst_offset..]);
                for (pixel, bytes) in pixels.iter().zip(output[dst_offset..].chunks_exact(2)) {
                    assert_eq!(bytes, pixel.to_be_bytes());
                }
                assert!(output[..dst_offset].iter().all(|&b| b == 0x55));
                assert_eq!(&output[output.len() - 4..], &[0x55; 4]);
            }
        }
    }

    #[test]
    fn numeric_pack_supports_unaligned_output_and_odd_width() {
        let input: Vec<u16> = (0..35).map(|i| i * 1777).collect();
        for offset in 0..4 {
            let mut output = vec![0x99; offset + 3 * 3 * 2 + 4];
            let count = pack_rgb565(&input, (7, 5), (1, 1, 3, 3), 3, &mut output[offset..]);
            let expected: Vec<u8> = (1..=3)
                .flat_map(|y| {
                    input[y * 7 + 1..y * 7 + 4]
                        .iter()
                        .flat_map(|p| p.to_be_bytes())
                })
                .collect();
            assert_eq!(&output[offset..offset + count], expected);
            assert_eq!(&output[output.len() - 4..], &[0x99; 4]);
        }
    }

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
