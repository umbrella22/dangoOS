//! Hardware-, allocator-, and LVGL-independent native RGB565 framebuffer model.
//!
//! Damage is tracked as one horizontal span per row. Disjoint writes on a row
//! intentionally upload the intervening pixels; no rectangle queue can overflow
//! and discard dirty pixels.

pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 480;
pub const PIXELS: usize = WIDTH * HEIGHT;

/// An inclusive panel rectangle. Public fields permit invalid construction;
/// prefer `checked` for external coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x1: u16,
    pub y1: u16,
    pub x2: u16,
    pub y2: u16,
}

impl Rect {
    /// Reject negative, reversed, or out-of-panel coordinates without clipping.
    pub const fn checked(x1: i32, y1: i32, x2: i32, y2: i32) -> Option<Self> {
        if x1 < 0 || y1 < 0 || x2 < x1 || y2 < y1 || x2 >= WIDTH as i32 || y2 >= HEIGHT as i32 {
            return None;
        }
        Some(Self {
            x1: x1 as u16,
            y1: y1 as u16,
            x2: x2 as u16,
            y2: y2 as u16,
        })
    }

    /// Inclusive width. Requires a valid rectangle.
    pub const fn width(self) -> usize {
        self.x2 as usize + 1 - self.x1 as usize
    }

    /// Inclusive height. Requires a valid rectangle.
    pub const fn height(self) -> usize {
        self.y2 as usize + 1 - self.y1 as usize
    }

    /// Pixel count. Requires a valid rectangle.
    pub const fn area(self) -> usize {
        self.width() * self.height()
    }

    const fn is_valid(self) -> bool {
        self.x1 <= self.x2
            && self.y1 <= self.y2
            && (self.x2 as usize) < WIDTH
            && (self.y2 as usize) < HEIGHT
    }
}

/// An inclusive panel rectangle used by the presentation handshake.
///
/// Coordinates are inclusive and panel-relative, matching the CO5300
/// `CASET`/`RASET` windows. `DirtyRect::EMPTY` is the canonical empty value;
/// any rectangle with `x0 > x1` or `y0 > y1` is also empty. Public fields
/// permit invalid construction; `is_valid` rejects rectangles outside the
/// panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}

impl DirtyRect {
    /// The whole panel.
    pub const FULL: Self = Self {
        x0: 0,
        y0: 0,
        x1: (WIDTH - 1) as u16,
        y1: (HEIGHT - 1) as u16,
    };

    /// Canonical empty rectangle.
    pub const EMPTY: Self = Self {
        x0: WIDTH as u16,
        y0: HEIGHT as u16,
        x1: 0,
        y1: 0,
    };

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

    /// Number of pixels covered; zero for an empty rectangle.
    pub const fn area(self) -> usize {
        self.width() * self.height()
    }

    /// Smallest rectangle covering both inputs; empty inputs are ignored.
    pub const fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self {
            x0: if self.x0 < other.x0 {
                self.x0
            } else {
                other.x0
            },
            y0: if self.y0 < other.y0 {
                self.y0
            } else {
                other.y0
            },
            x1: if self.x1 > other.x1 {
                self.x1
            } else {
                other.x1
            },
            y1: if self.y1 > other.y1 {
                self.y1
            } else {
                other.y1
            },
        }
    }

    /// Round x outward to four-pixel boundaries, clamped to the panel.
    ///
    /// The CO5300 QSPI path requires the x window to be a multiple of four.
    /// Empty rectangles stay empty; y is unchanged.
    pub const fn align_x4(self) -> Self {
        if self.is_empty() {
            return Self::EMPTY;
        }
        let right = self.x1 | 3;
        let right = if right > (WIDTH - 1) as u16 {
            (WIDTH - 1) as u16
        } else {
            right
        };
        Self {
            x0: self.x0 & !3,
            y0: self.y0,
            x1: right,
            y1: self.y1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyError {
    InvalidArea,
    FramebufferTooShort,
    StrideTooShort,
    SourceTooShort,
    LengthOverflow,
}

/// Copy raw native-endian RGB565 bytes into the fixed-stride panel framebuffer.
///
/// `stride` is the source row stride in **bytes**, not pixels. Only pixel bytes
/// are read; padding after the final row need not exist. The target must contain
/// at least `PIXELS` elements; extra elements remain untouched. All validation
/// occurs before mutation, including public-field rectangles. No color
/// conversion is performed. An unaligned source start is supported.
pub fn copy_rgb565(
    framebuffer: &mut [u16],
    area: Rect,
    source: &[u8],
    stride: usize,
) -> Result<(), CopyError> {
    if !area.is_valid() {
        return Err(CopyError::InvalidArea);
    }
    if framebuffer.len() < PIXELS {
        return Err(CopyError::FramebufferTooShort);
    }
    let row_bytes = area.width() * core::mem::size_of::<u16>();
    if stride < row_bytes {
        return Err(CopyError::StrideTooShort);
    }
    let required = (area.height() - 1)
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add(row_bytes))
        .ok_or(CopyError::LengthOverflow)?;
    if source.len() < required {
        return Err(CopyError::SourceTooShort);
    }

    for row in 0..area.height() {
        let source_offset = row * stride;
        let target_offset = (area.y1 as usize + row) * WIDTH + area.x1 as usize;
        // SAFETY: Validated panel bounds and framebuffer length place the entire
        // target row in initialized u16 storage. Validated source length covers
        // every row and proves offset arithmetic cannot overflow. Copying as u8
        // requires no source alignment, unlike casting to &[u16]. All u16 bit
        // patterns are valid and native byte order is preserved. Distinct safe
        // &mut [u16] and &[u8] borrows guarantee these regions cannot overlap.
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.as_ptr().add(source_offset),
                framebuffer.as_mut_ptr().add(target_offset).cast::<u8>(),
                row_bytes,
            );
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Span {
    x1: u16,
    x2: u16,
}

impl Span {
    const EMPTY: Self = Self {
        x1: WIDTH as u16,
        x2: 0,
    };

    const fn is_pending(self) -> bool {
        self.x1 <= self.x2
    }
}

/// Fixed 1,920-byte dirty-span state (four bytes per panel row).
///
/// Sequential transport contract: `next_chunk` only peeks. Submit that exact
/// rectangle and call `complete` with its transport result before rendering or
/// marking again. Do not have multiple chunks in flight. Failed transfers leave
/// all damage pending. Successful transfers clear only rows whose current span
/// matches the submitted span, preserving any horizontal expansion.
///
/// Span comparison is not a generation counter: new writes within an identical
/// span cannot be distinguished and would be cleared by `complete`. Thus the
/// expansion safeguard does NOT permit concurrent/interleaved rendering during
/// transport. The caller must uphold the sequential contract above.
pub struct Damage {
    rows: [Span; HEIGHT],
}

impl Damage {
    pub const fn new() -> Self {
        Self {
            rows: [Span::EMPTY; HEIGHT],
        }
    }

    /// Expand each touched row, rounding horizontal bounds outward to four
    /// pixels within the panel. Invalid public-field rectangles are ignored.
    pub fn mark(&mut self, area: Rect) {
        if !area.is_valid() {
            return;
        }
        let x1 = area.x1 & !3;
        let x2 = core::cmp::min(area.x2 | 3, (WIDTH - 1) as u16);
        for span in &mut self.rows[area.y1 as usize..=area.y2 as usize] {
            if span.is_pending() {
                span.x1 = core::cmp::min(span.x1, x1);
                span.x2 = core::cmp::max(span.x2, x2);
            } else {
                *span = Span { x1, x2 };
            }
        }
    }

    pub fn is_pending(&self) -> bool {
        self.rows.iter().any(|span| span.is_pending())
    }

    /// Peek at the earliest pending run of identical spans, limited to
    /// `max_rows`. Zero rows yields `None` without changing pending damage.
    pub fn next_chunk(&self, max_rows: u16) -> Option<Rect> {
        if max_rows == 0 {
            return None;
        }
        let y1 = self.rows.iter().position(|span| span.is_pending())?;
        let span = self.rows[y1];
        let limit = core::cmp::min(HEIGHT, y1 + max_rows as usize);
        let mut end = y1 + 1;
        while end < limit && self.rows[end] == span {
            end += 1;
        }
        Some(Rect {
            x1: span.x1,
            y1: y1 as u16,
            x2: span.x2,
            y2: (end - 1) as u16,
        })
    }

    /// Acknowledge the exact rectangle returned by `next_chunk`. Failure is a
    /// no-op. On success, expanded row spans survive. Invalid areas are ignored.
    pub fn complete(&mut self, area: Rect, success: bool) {
        if !success || !area.is_valid() {
            return;
        }
        let submitted = Span {
            x1: area.x1,
            x2: area.x2,
        };
        for span in &mut self.rows[area.y1 as usize..=area.y2 as usize] {
            if *span == submitted {
                *span = Span::EMPTY;
            }
        }
    }

    /// Pending upload pixels, including alignment and gaps bridged by a row
    /// span, not just pixels explicitly written by the caller.
    pub fn pending_pixels(&self) -> usize {
        self.rows
            .iter()
            .filter(|span| span.is_pending())
            .map(|span| span.x2 as usize + 1 - span.x1 as usize)
            .sum()
    }
}

impl Default for Damage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x1: i32, y1: i32, x2: i32, y2: i32) -> Rect {
        Rect::checked(x1, y1, x2, y2).unwrap()
    }

    fn bytes(pixels: &[u16]) -> Vec<u8> {
        pixels
            .iter()
            .flat_map(|pixel| pixel.to_ne_bytes())
            .collect()
    }

    fn assert_copy_error(area: Rect, source: &[u8], stride: usize, error: CopyError) {
        let mut frame = vec![0x5aa5; PIXELS];
        assert_eq!(copy_rgb565(&mut frame, area, source, stride), Err(error));
        assert!(frame.iter().all(|&pixel| pixel == 0x5aa5));
    }

    #[test]
    fn inclusive_rect_dimensions_and_strict_bounds() {
        let full = rect(0, 0, 479, 479);
        assert_eq!(
            (full.width(), full.height(), full.area()),
            (480, 480, PIXELS)
        );
        assert_eq!(rect(479, 479, 479, 479).area(), 1);
        for (x1, y1, x2, y2) in [
            (-1, 0, 1, 1),
            (0, -1, 1, 1),
            (0, 0, -1, 1),
            (0, 0, 1, -1),
            (2, 0, 1, 1),
            (0, 2, 1, 1),
            (0, 0, 480, 1),
            (0, 0, 1, 480),
            (480, 0, 480, 0),
            (0, 480, 0, 480),
            (i32::MIN, 0, 0, 0),
            (0, 0, i32::MAX, i32::MAX),
        ] {
            assert_eq!(Rect::checked(x1, y1, x2, y2), None);
        }
    }

    #[test]
    fn native_primary_colors_stride_padding_and_unaligned_source() {
        let colors = [0xf800u16, 0x07e0, 0x001f, 0xffff, 0x0000, 0x1234];
        // Offset a u16-aligned allocation by one byte, ensuring misalignment.
        let mut storage = [0u16; 9];
        let raw = unsafe {
            // SAFETY: Initialized u16 storage permits byte access; this slice
            // covers its exact byte length and holds its exclusive borrow.
            core::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), 18)
        };
        raw.fill(0xee);
        raw[1..7].copy_from_slice(&bytes(&colors[..3]));
        raw[10..16].copy_from_slice(&bytes(&colors[3..]));
        assert_ne!(raw[1..].as_ptr() as usize % core::mem::align_of::<u16>(), 0);
        let mut frame = vec![0x5aa5; PIXELS];
        // Odd stride, with no trailing padding after the final row.
        copy_rgb565(&mut frame, rect(7, 9, 9, 10), &raw[1..16], 9).unwrap();
        assert_eq!(&frame[9 * WIDTH + 7..9 * WIDTH + 10], &colors[..3]);
        assert_eq!(&frame[10 * WIDTH + 7..10 * WIDTH + 10], &colors[3..]);
    }

    #[test]
    fn invalid_public_rectangles_do_not_mutate() {
        for area in [
            Rect {
                x1: 0,
                y1: 0,
                x2: 480,
                y2: 1,
            },
            Rect {
                x1: 0,
                y1: 0,
                x2: 1,
                y2: 480,
            },
            Rect {
                x1: 2,
                y1: 0,
                x2: 1,
                y2: 1,
            },
            Rect {
                x1: 0,
                y1: 2,
                x2: 1,
                y2: 1,
            },
            Rect {
                x1: u16::MAX,
                y1: 0,
                x2: u16::MAX,
                y2: 0,
            },
            Rect {
                x1: 0,
                y1: u16::MAX,
                x2: 0,
                y2: u16::MAX,
            },
        ] {
            assert_copy_error(area, &[0; 16], 8, CopyError::InvalidArea);
        }
    }

    #[test]
    fn short_stride_and_source_and_overflow_do_not_mutate() {
        let area = rect(10, 20, 12, 22);
        assert_copy_error(area, &[0; 32], 5, CopyError::StrideTooShort);
        // Two padded strides plus the final row require 22 bytes.
        assert_copy_error(area, &[0; 21], 8, CopyError::SourceTooShort);
        assert_copy_error(area, &[], usize::MAX, CopyError::LengthOverflow);
        assert_copy_error(rect(0, 0, 0, 1), &[], usize::MAX, CopyError::LengthOverflow);
    }

    #[test]
    fn short_framebuffer_does_not_mutate() {
        let mut frame = vec![0x5aa5; PIXELS - 1];
        assert_eq!(
            copy_rgb565(&mut frame, rect(0, 0, 0, 0), &[0; 2], 2),
            Err(CopyError::FramebufferTooShort)
        );
        assert!(frame.iter().all(|&pixel| pixel == 0x5aa5));
    }

    #[test]
    fn partial_copy_preserves_all_outside_pixels_and_extra_storage() {
        let mut frame = vec![0x5aa5; PIXELS + 3];
        let colors = [1u16, 2, 3, 4, 5, 6];
        copy_rgb565(&mut frame, rect(477, 478, 479, 479), &bytes(&colors), 6).unwrap();
        for (i, &pixel) in frame.iter().enumerate() {
            let x = i % WIDTH;
            let y = i / WIDTH;
            let expected = if y >= 478 && y < HEIGHT && x >= 477 {
                colors[(y - 478) * 3 + x - 477]
            } else {
                0x5aa5
            };
            assert_eq!(pixel, expected, "pixel {i}");
        }
    }

    #[test]
    fn full_frame_copy() {
        let mut frame = vec![0; PIXELS];
        let pixels: Vec<u16> = (0..PIXELS).map(|i| i as u16).collect();
        copy_rgb565(&mut frame, rect(0, 0, 479, 479), &bytes(&pixels), WIDTH * 2).unwrap();
        assert_eq!(frame, pixels);
    }

    #[test]
    fn full_frame_marks_coalesce_into_thirty_unique_chunks() {
        let mut damage = Damage::new();
        assert!(!damage.is_pending());
        assert_eq!(damage.pending_pixels(), 0);
        assert_eq!(damage.next_chunk(16), None);
        // Mark in reverse order and repeat a whole-frame mark: no duplicates.
        for y in (0..HEIGHT).rev() {
            damage.mark(rect(0, y as i32, 479, y as i32));
        }
        damage.mark(rect(0, 0, 479, 479));
        assert_eq!(damage.pending_pixels(), PIXELS);
        let mut uploaded = vec![false; PIXELS];
        let mut count = 0;
        while let Some(chunk) = damage.next_chunk(16) {
            assert_eq!(chunk, rect(0, count * 16, 479, count * 16 + 15));
            assert_eq!(chunk.height(), 16);
            assert_eq!(damage.next_chunk(16), Some(chunk));
            for y in chunk.y1 as usize..=chunk.y2 as usize {
                for x in chunk.x1 as usize..=chunk.x2 as usize {
                    let pixel = &mut uploaded[y * WIDTH + x];
                    assert!(!*pixel, "duplicate upload at ({}, {})", x, y);
                    *pixel = true;
                }
            }
            damage.complete(chunk, true);
            count += 1;
            assert_eq!(
                damage.pending_pixels(),
                PIXELS - count as usize * 16 * WIDTH
            );
        }
        assert_eq!(count, 30);
        assert!(uploaded.iter().all(|&pixel| pixel));
        assert!(!damage.is_pending());
    }

    #[test]
    fn partial_last_chunk_and_zero_row_limit() {
        let mut damage = Damage::new();
        damage.mark(rect(8, 460, 15, 479));
        assert_eq!(damage.next_chunk(0), None);
        assert_eq!(damage.pending_pixels(), 8 * 20);
        let first = damage.next_chunk(16).unwrap();
        assert_eq!(first, rect(8, 460, 15, 475));
        damage.complete(first, true);
        let last = damage.next_chunk(16).unwrap();
        assert_eq!(last, rect(8, 476, 15, 479));
        assert_eq!(last.height(), 4);
        damage.complete(last, true);
        assert_eq!(damage.pending_pixels(), 0);
    }

    #[test]
    fn horizontal_alignment_including_right_edge() {
        for (area, expected) in [
            (rect(0, 0, 0, 0), rect(0, 0, 3, 0)),
            (rect(5, 1, 6, 1), rect(4, 1, 7, 1)),
            (rect(3, 2, 4, 2), rect(0, 2, 7, 2)),
            (rect(478, 479, 479, 479), rect(476, 479, 479, 479)),
            (rect(479, 0, 479, 0), rect(476, 0, 479, 0)),
            (rect(0, 0, 479, 0), rect(0, 0, 479, 0)),
        ] {
            let mut damage = Damage::new();
            damage.mark(area);
            assert_eq!(damage.next_chunk(u16::MAX), Some(expected));
            assert_eq!(damage.pending_pixels(), expected.area());
        }
    }

    #[test]
    fn failed_chunk_is_retained_then_retry_succeeds() {
        let mut damage = Damage::new();
        damage.mark(rect(17, 8, 21, 10));
        let chunk = damage.next_chunk(16).unwrap();
        let pending = damage.pending_pixels();
        damage.complete(chunk, false);
        assert!(damage.is_pending());
        assert_eq!(damage.pending_pixels(), pending);
        assert_eq!(damage.next_chunk(16), Some(chunk));
        damage.complete(chunk, true);
        assert!(!damage.is_pending());
        assert_eq!(damage.next_chunk(16), None);
    }

    #[test]
    fn disjoint_damage_remains_separate_across_clean_rows() {
        let mut damage = Damage::new();
        damage.mark(rect(1, 1, 2, 2));
        damage.mark(rect(101, 4, 102, 5));
        assert_eq!(damage.pending_pixels(), 16);
        let first = damage.next_chunk(16).unwrap();
        assert_eq!(first, rect(0, 1, 3, 2));
        damage.complete(first, true);
        let second = damage.next_chunk(16).unwrap();
        assert_eq!(second, rect(100, 4, 103, 5));
        damage.complete(second, true);
        assert!(!damage.is_pending());
    }

    #[test]
    fn adjacent_rows_with_different_spans_do_not_coalesce() {
        let mut damage = Damage::new();
        damage.mark(rect(0, 0, 3, 0));
        damage.mark(rect(4, 1, 7, 1));
        assert_eq!(damage.next_chunk(16), Some(rect(0, 0, 3, 0)));
        damage.complete(rect(0, 0, 3, 0), true);
        assert_eq!(damage.next_chunk(16), Some(rect(4, 1, 7, 1)));
    }

    #[test]
    fn stale_completion_preserves_expanded_rows_only() {
        let mut damage = Damage::new();
        damage.mark(rect(5, 10, 6, 12));
        let submitted = damage.next_chunk(16).unwrap();
        assert_eq!(submitted, rect(4, 10, 7, 12));
        // Test defensive span comparison; interleaved rendering is otherwise
        // forbidden by the documented transport contract.
        damage.mark(rect(10, 11, 10, 11)); // Expand right on row 11.
        damage.mark(rect(0, 12, 0, 12)); // Expand left on row 12.
        damage.complete(submitted, true);
        assert_eq!(damage.pending_pixels(), 16);
        let right = damage.next_chunk(16).unwrap();
        assert_eq!(right, rect(4, 11, 11, 11));
        damage.complete(right, true);
        let left = damage.next_chunk(16).unwrap();
        assert_eq!(left, rect(0, 12, 7, 12));
        damage.complete(left, true);
        assert!(!damage.is_pending());
    }

    #[test]
    fn disjoint_writes_on_one_row_expand_without_discarding_damage() {
        let mut damage = Damage::new();
        damage.mark(rect(401, 100, 402, 100));
        damage.mark(rect(5, 100, 6, 100));
        assert_eq!(damage.next_chunk(16), Some(rect(4, 100, 403, 100)));
        assert_eq!(damage.pending_pixels(), 400);
        damage.mark(rect(201, 100, 202, 100));
        assert_eq!(damage.pending_pixels(), 400);
    }

    #[test]
    fn invalid_damage_rectangles_are_noops_and_state_is_small() {
        assert_eq!(core::mem::size_of::<Damage>(), HEIGHT * 4);
        let mut damage = Damage::default();
        let invalid = Rect {
            x1: 0,
            y1: 0,
            x2: 480,
            y2: 480,
        };
        damage.mark(invalid);
        assert!(!damage.is_pending());
        damage.mark(rect(0, 0, 3, 0));
        damage.complete(invalid, true);
        assert_eq!(damage.pending_pixels(), 4);
    }

    #[test]
    fn dirty_rect_constants_union_and_dimensions() {
        assert_eq!(DirtyRect::FULL.area(), PIXELS);
        assert!(DirtyRect::FULL.is_valid());
        assert!(DirtyRect::EMPTY.is_empty());
        assert_eq!(DirtyRect::EMPTY.area(), 0);
        assert_eq!(DirtyRect::FULL.union(DirtyRect::EMPTY), DirtyRect::FULL);
        assert_eq!(DirtyRect::EMPTY.union(DirtyRect::FULL), DirtyRect::FULL);
        let a = DirtyRect {
            x0: 10,
            y0: 10,
            x1: 20,
            y1: 20,
        };
        let b = DirtyRect {
            x0: 110,
            y0: 2,
            x1: 120,
            y1: 5,
        };
        let expected = DirtyRect {
            x0: 10,
            y0: 2,
            x1: 120,
            y1: 20,
        };
        assert_eq!(a.union(b), expected);
        assert_eq!(b.union(a), expected);
        assert_eq!((a.width(), a.height(), a.area()), (11, 11, 121));
    }

    #[test]
    fn dirty_rect_alignment_rounds_outward_and_clamps() {
        for (area, expected) in [
            (DirtyRect::FULL, DirtyRect::FULL),
            (DirtyRect::EMPTY, DirtyRect::EMPTY),
            (
                DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: 0,
                    y1: 0,
                },
                DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: 3,
                    y1: 0,
                },
            ),
            (
                DirtyRect {
                    x0: 5,
                    y0: 1,
                    x1: 6,
                    y1: 1,
                },
                DirtyRect {
                    x0: 4,
                    y0: 1,
                    x1: 7,
                    y1: 1,
                },
            ),
            (
                DirtyRect {
                    x0: 479,
                    y0: 0,
                    x1: 479,
                    y1: 0,
                },
                DirtyRect {
                    x0: 476,
                    y0: 0,
                    x1: 479,
                    y1: 0,
                },
            ),
            (
                DirtyRect {
                    x0: 478,
                    y0: 479,
                    x1: 479,
                    y1: 479,
                },
                DirtyRect {
                    x0: 476,
                    y0: 479,
                    x1: 479,
                    y1: 479,
                },
            ),
        ] {
            let aligned = area.align_x4();
            assert_eq!(aligned, expected, "{area:?}");
            if aligned.is_empty() {
                continue;
            }
            assert_eq!(aligned.x0 % 4, 0);
            assert_eq!((aligned.x1 + 1) % 4, 0);
            assert_eq!(aligned.y0, area.y0);
            assert_eq!(aligned.y1, area.y1);
        }
    }

    #[test]
    fn dirty_rect_invalid_rectangles_are_rejected() {
        for area in [
            DirtyRect {
                x0: 2,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            DirtyRect {
                x0: 0,
                y0: 2,
                x1: 1,
                y1: 1,
            },
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: 480,
                y1: 1,
            },
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 480,
            },
            DirtyRect {
                x0: 500,
                y0: 0,
                x1: 500,
                y1: 0,
            },
        ] {
            assert!(!area.is_valid(), "{area:?}");
        }
        assert!(!DirtyRect::EMPTY.is_valid());
    }
}
