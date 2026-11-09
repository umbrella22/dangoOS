//! BAN1 grayscale animation: timed XOR patches, independently zlib compressed.
//! Parsing/decoding allocate nothing; the caller owns the current frame and
//! one scratch area. Index windows are exclusive and four-pixel aligned.

pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 480;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT / 2;
const HEADER: usize = 16;
const ENTRY: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Header,
    Index,
    Buffer,
    Decode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}
impl Window {
    pub fn width(self) -> usize {
        (self.x1 - self.x0) as usize
    }
    pub fn height(self) -> usize {
        (self.y1 - self.y0) as usize
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub duration_ms: u32,
    pub window: Option<Window>,
}
pub struct Animation<'a> {
    bytes: &'a [u8],
    count: usize,
    next: usize,
}
fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
impl<'a> Animation<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER
            || &bytes[..4] != b"BAN1"
            || u16_at(bytes, 4) as usize != WIDTH
            || u16_at(bytes, 6) as usize != HEIGHT
            || u16_at(bytes, 10) != 4
            || u32_at(bytes, 12) as usize != FRAME_BYTES
        {
            return Err(Error::Header);
        }
        let count = u16_at(bytes, 8) as usize;
        let mut end = HEADER + count * ENTRY;
        if count == 0 || end > bytes.len() {
            return Err(Error::Index);
        }
        for index in 0..count {
            let at = HEADER + index * ENTRY;
            let offset = u32_at(bytes, at) as usize;
            let len = u32_at(bytes, at + 4) as usize;
            let w = Window {
                x0: u16_at(bytes, at + 12),
                y0: u16_at(bytes, at + 14),
                x1: u16_at(bytes, at + 16),
                y1: u16_at(bytes, at + 18),
            };
            let valid = if len == 0 {
                w == Window {
                    x0: 0,
                    y0: 0,
                    x1: 0,
                    y1: 0,
                }
            } else {
                w.x0 < w.x1
                    && w.y0 < w.y1
                    && w.x1 as usize <= WIDTH
                    && w.y1 as usize <= HEIGHT
                    && w.x0 % 4 == 0
                    && w.x1 % 4 == 0
            };
            if offset != end || u32_at(bytes, at + 8) == 0 || !valid {
                return Err(Error::Index);
            }
            end = offset.checked_add(len).ok_or(Error::Index)?;
            if end > bytes.len() {
                return Err(Error::Index);
            }
        }
        if end != bytes.len() {
            return Err(Error::Index);
        }
        Ok(Self {
            bytes,
            count,
            next: 0,
        })
    }
    pub fn frames(&self) -> usize {
        self.count
    }
    pub fn finished(&self) -> bool {
        self.next == self.count
    }
    pub fn advance(&mut self, current: &mut [u8], scratch: &mut [u8]) -> Result<Frame, Error> {
        if current.len() != FRAME_BYTES || scratch.len() != FRAME_BYTES {
            return Err(Error::Buffer);
        }
        if self.finished() {
            return Err(Error::Index);
        }
        let at = HEADER + self.next * ENTRY;
        let offset = u32_at(self.bytes, at) as usize;
        let len = u32_at(self.bytes, at + 4) as usize;
        let duration_ms = u32_at(self.bytes, at + 8);
        let w = Window {
            x0: u16_at(self.bytes, at + 12),
            y0: u16_at(self.bytes, at + 14),
            x1: u16_at(self.bytes, at + 16),
            y1: u16_at(self.bytes, at + 18),
        };
        if len != 0 {
            let needed = w.width() * w.height() / 2;
            let decoded = miniz_oxide::inflate::decompress_slice_iter_to_slice(
                &mut scratch[..needed],
                core::iter::once(&self.bytes[offset..offset + len]),
                true,
                false,
            )
            .map_err(|_| Error::Decode)?;
            if decoded != needed {
                return Err(Error::Decode);
            }
        }
        if self.next == 0 {
            current.fill(0);
        }
        if len != 0 {
            let stride = w.width() / 2;
            for row in 0..w.height() {
                let at = (w.y0 as usize + row) * WIDTH / 2 + w.x0 as usize / 2;
                for (old, delta) in current[at..at + stride]
                    .iter_mut()
                    .zip(&scratch[row * stride..(row + 1) * stride])
                {
                    *old ^= *delta;
                }
            }
        }
        self.next += 1;
        Ok(Frame {
            duration_ms,
            window: (len != 0).then_some(w),
        })
    }
}

pub const fn gray565(index: u8) -> u16 {
    let value = (index & 15) as u16 * 17;
    ((value >> 3) << 11) | ((value >> 2) << 5) | (value >> 3)
}

// One packed byte expands to two little-endian RGB565 pixels. Keep this in
// flash so the hot path reads every source byte once and allocates nothing.
static RGB565_PAIRS: [[u8; 4]; 256] = {
    let mut pairs = [[0; 4]; 256];
    let mut index = 0;
    while index < pairs.len() {
        let left = gray565((index >> 4) as u8).to_le_bytes();
        let right = gray565(index as u8).to_le_bytes();
        pairs[index] = [left[0], left[1], right[0], right[1]];
        index += 1;
    }
    pairs
};

/// Expand high-nibble-first gray4 into the display shadow's RGB565 byte order.
/// An invalid output size leaves the destination untouched.
pub fn expand_rgb565(packed: &[u8], out: &mut [u8]) -> Result<(), Error> {
    if packed.len().checked_mul(4) != Some(out.len()) {
        return Err(Error::Buffer);
    }
    for (pair, &indices) in out.chunks_exact_mut(4).zip(packed) {
        pair.copy_from_slice(&RGB565_PAIRS[indices as usize]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    const BUNDLE: &[u8] = include_bytes!("../assets/boot.ban");
    #[test]
    fn packed_pixels_keep_nibble_order_and_little_endian_rgb565() {
        let mut out = [0; 12];
        expand_rgb565(&[0x0f, 0xf0, 0x78], &mut out).unwrap();
        assert_eq!(
            out,
            [0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0xae, 0x73, 0x51, 0x8c]
        );
        assert_eq!(expand_rgb565(&[0x00], &mut out), Err(Error::Buffer));
        assert_eq!(&out[..4], &[0x00, 0x00, 0xff, 0xff]);
    }
    #[test]
    fn all_boot_frames_present_through_aligned_windows_and_keep_their_timing() {
        let mut a = Animation::parse(BUNDLE).unwrap();
        let mut current = vec![0; FRAME_BYTES];
        let mut scratch = vec![0; FRAME_BYTES];
        let mut screen = vec![0; FRAME_BYTES];
        let mut duration = 0;
        assert_eq!(a.frames(), 33);
        while !a.finished() {
            let frame = a.advance(&mut current, &mut scratch).unwrap();
            duration += frame.duration_ms;
            if let Some(w) = frame.window {
                for y in w.y0..w.y1 {
                    let at = y as usize * WIDTH / 2 + w.x0 as usize / 2;
                    let end = at + w.width() / 2;
                    screen[at..end].copy_from_slice(&current[at..end]);
                }
            }
            assert_eq!(screen, current);
        }
        assert_eq!(duration, 1320);
        assert_eq!(gray565(0), 0);
        assert_eq!(gray565(15), 0xffff);
        assert!(current.iter().any(|&x| x != 0));
    }
    #[test]
    fn truncated_misaligned_and_corrupt_frames_do_not_modify_the_displayed_frame() {
        for end in [0, 15, HEADER + 4, BUNDLE.len() - 1] {
            assert!(Animation::parse(&BUNDLE[..end]).is_err());
        }
        let mut bad = BUNDLE.to_vec();
        // The first nonempty frame's x window must stay aligned.
        let index = (0..Animation::parse(BUNDLE).unwrap().frames())
            .find(|&i| u32_at(BUNDLE, HEADER + i * ENTRY + 4) != 0)
            .unwrap();
        bad[HEADER + index * ENTRY + 12] |= 1;
        assert!(Animation::parse(&bad).is_err());
        let mut bad = BUNDLE.to_vec();
        let at = HEADER + index * ENTRY;
        let offset = u32_at(&bad, at) as usize;
        bad[offset] ^= 0xff;
        let mut a = Animation::parse(&bad).unwrap();
        let mut current = vec![0; FRAME_BYTES];
        let mut scratch = vec![0; FRAME_BYTES];
        for _ in 0..index {
            a.advance(&mut current, &mut scratch).unwrap();
        }
        let before = current.clone();
        assert_eq!(a.advance(&mut current, &mut scratch), Err(Error::Decode));
        assert_eq!(current, before);
    }

    #[test]
    fn application_first_frame_matches_bootloader_static_pixels() {
        // Compare two independently packed representations at their display
        // boundary. A changed logo or grayscale mapping must update both.
        let source = include_str!("../../../../bootloader/bootloader_components/main/splash.inc");
        let body = source.split_once('{').unwrap().1.split_once('}').unwrap().0;
        let bytes: alloc::vec::Vec<u8> = body
            .split(',')
            .filter_map(|word| {
                let word = word.trim();
                (!word.is_empty())
                    .then(|| u8::from_str_radix(word.trim_start_matches("0x"), 16).unwrap())
            })
            .collect();
        let mut bootloader = alloc::vec::Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let code = bytes[at];
            at += 1;
            let mut count = (code & 15) as usize + 1;
            if count == 16 {
                let mut shift = 0;
                loop {
                    let extension = bytes[at];
                    at += 1;
                    count += ((extension & 127) as usize) << shift;
                    if extension & 128 == 0 {
                        break;
                    }
                    shift += 7;
                }
            }
            bootloader.resize(bootloader.len() + count, code >> 4);
        }
        assert_eq!(bootloader.len(), WIDTH * HEIGHT);
        let mut animation = Animation::parse(BUNDLE).unwrap();
        let mut current = vec![0; FRAME_BYTES];
        let mut scratch = vec![0; FRAME_BYTES];
        animation.advance(&mut current, &mut scratch).unwrap();
        for (pair, pixels) in current.iter().zip(bootloader.chunks_exact(2)) {
            assert_eq!(*pair, pixels[0] << 4 | pixels[1]);
        }
    }
}
