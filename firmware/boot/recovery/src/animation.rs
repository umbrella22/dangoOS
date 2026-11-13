//! Indexed, independently compressed animation frames. Decoding reuses one
//! 115200-byte work area; a damaged frame falls back to the static screen.
pub const WIDTH: usize = 480;
pub const HEIGHT: usize = 480;
pub const WORK_BYTES: usize = WIDTH * HEIGHT / 2;
const HEADER: usize = 48;
const INDEX: usize = 16;
/// Bounds of the picture above the fixed footer, aligned to the panel's
/// four-column, two-row QSPI windows. No previous pixel buffer is needed.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
}
impl Bounds {
    pub fn union(self, other: Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }
}
pub fn visible_bounds(frame: &[u8]) -> Option<Bounds> {
    assert_eq!(frame.len(), WORK_BYTES);
    let mut rect: Option<Bounds> = None;
    for (y, row) in frame[..380 * 240].chunks_exact(240).enumerate() {
        if let Some(left) = row.iter().position(|&b| b != 0) {
            let right = row.iter().rposition(|&b| b != 0).unwrap();
            let r = rect.get_or_insert(Bounds {
                x0: 476,
                y0: y as u16 & !1,
                x1: 3,
                y1: y as u16,
            });
            r.x0 = r.x0.min((left as u16 * 2) & !3);
            r.x1 = r.x1.max((right as u16 * 2 + 1) | 3);
            r.y1 = y as u16 | 1;
        }
    }
    rect
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Header,
    Index,
    Decode,
    Crc,
    WorkBuffer,
}
pub struct Animation<'a> {
    bytes: &'a [u8],
    count: usize,
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
impl<'a> Animation<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER
            || &bytes[..4] != b"RAN1"
            || u16::from_le_bytes(bytes[4..6].try_into().unwrap()) as usize != WIDTH
            || u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as usize != HEIGHT
            || u16::from_le_bytes(bytes[10..12].try_into().unwrap()) != 16
            || u32_at(bytes, 12) as usize != WORK_BYTES
        {
            return Err(Error::Header);
        }
        let count = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
        if count == 0 || HEADER + count * INDEX > bytes.len() {
            return Err(Error::Index);
        }
        let animation = Self { bytes, count };
        let mut end = HEADER + count * INDEX;
        for i in 0..count {
            let (offset, len, duration, _) = animation.entry(i)?;
            if offset != end || duration == 0 {
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
        Ok(animation)
    }
    pub fn frames(&self) -> usize {
        self.count
    }
    fn entry(&self, index: usize) -> Result<(usize, usize, u32, u32), Error> {
        if index >= self.count {
            return Err(Error::Index);
        }
        let at = HEADER + index * INDEX;
        Ok((
            u32_at(self.bytes, at) as usize,
            u32_at(self.bytes, at + 4) as usize,
            u32_at(self.bytes, at + 8),
            u32_at(self.bytes, at + 12),
        ))
    }
    pub fn color(&self, index: u8) -> u16 {
        let at = 16 + (index & 15) as usize * 2;
        u16::from_le_bytes(self.bytes[at..at + 2].try_into().unwrap())
    }
    pub fn decode(&self, index: usize, out: &mut [u8]) -> Result<u32, Error> {
        if out.len() != WORK_BYTES {
            return Err(Error::WorkBuffer);
        }
        let (offset, len, duration, crc) = self.entry(index)?;
        let n = miniz_oxide::inflate::decompress_slice_iter_to_slice(
            out,
            core::iter::once(&self.bytes[offset..offset + len]),
            true,
            false,
        )
        .map_err(|_| Error::Decode)?;
        if n != WORK_BYTES {
            return Err(Error::Decode);
        }
        if crate::protocol::crc32(out) != crc {
            return Err(Error::Crc);
        }
        Ok(duration)
    }
}
#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::{vec, vec::Vec};
    #[test]
    fn every_bundled_frame_presents_exactly_through_aligned_damage_windows() {
        let bytes = include_bytes!("assets/animation.ran");
        let a = Animation::parse(bytes).unwrap();
        assert_eq!(a.color(0), 0, "damage clearing requires black index zero");
        let mut work = vec![0; WORK_BYTES];
        let mut presented = vec![0; WORK_BYTES];
        let mut previous: Option<Bounds> = None;
        let mut duration = 0;
        for index in 0..a.frames() {
            duration += a.decode(index, &mut work).unwrap();
            let current = visible_bounds(&work);
            let damage = match (previous, current) {
                (Some(a), Some(b)) => Some(a.union(b)),
                (a, b) => a.or(b),
            };
            if let Some(r) = damage {
                assert_eq!(r.x0 % 4, 0);
                assert_eq!((r.x1 - r.x0 + 1) % 4, 0);
                assert_eq!(r.y0 % 2, 0);
                assert_eq!((r.y1 - r.y0 + 1) % 2, 0);
                assert!(r.x1 < 480 && r.y1 < 380);
                for row in r.y0..=r.y1 {
                    let at = row as usize * 240;
                    let range = at + r.x0 as usize / 2..at + (r.x1 as usize + 1) / 2;
                    presented[range.clone()].copy_from_slice(&work[range]);
                }
            }
            assert_eq!(&presented[..380 * 240], &work[..380 * 240], "frame {index}");
            previous = current;
        }
        assert_eq!(a.frames(), 126);
        assert_eq!(duration, 3960);
    }
    #[test]
    fn single_odd_rows_include_their_unchanged_partner_without_entering_footer() {
        for y in [1, 378, 379] {
            let mut frame = vec![0; WORK_BYTES];
            frame[y * WIDTH / 2 + 1] = 0x10;
            let r = visible_bounds(&frame).unwrap();
            assert_eq!((r.y0, r.y1), (y as u16 & !1, y as u16 | 1));
            assert!(r.y1 < 380);
        }
    }
    fn fixture() -> Vec<u8> {
        let mut header = Vec::from(*b"RAN1");
        header.extend_from_slice(&480u16.to_le_bytes());
        header.extend_from_slice(&480u16.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(&(WORK_BYTES as u32).to_le_bytes());
        header.extend_from_slice(&[0; 32]);
        let frames = [vec![0; WORK_BYTES], vec![0x17; WORK_BYTES]];
        let mut payload = Vec::new();
        for (i, frame) in frames.iter().enumerate() {
            let z = miniz_oxide::deflate::compress_to_vec_zlib(frame, 6);
            for n in [
                (HEADER + 2 * INDEX + payload.len()) as u32,
                z.len() as u32,
                (i as u32 + 1) * 30,
                crate::protocol::crc32(frame),
            ] {
                header.extend_from_slice(&n.to_le_bytes());
            }
            payload.extend(z);
        }
        header.extend(payload);
        header
    }
    #[test]
    fn exact_frame_order_timing_and_reusable_work_buffer() {
        let bytes = fixture();
        let a = Animation::parse(&bytes).unwrap();
        let mut work = vec![0; WORK_BYTES];
        assert_eq!(a.frames(), 2);
        assert_eq!(a.decode(0, &mut work), Ok(30));
        assert!(work.iter().all(|&b| b == 0));
        assert_eq!(a.decode(1, &mut work), Ok(60));
        assert!(work.iter().all(|&b| b == 0x17));
        assert_eq!(a.decode(2, &mut work), Err(Error::Index));
    }
    #[test]
    fn corrupt_offsets_frames_and_work_sizes_fail_without_partial_acceptance() {
        let mut bytes = fixture();
        bytes[48] ^= 1;
        assert!(matches!(Animation::parse(&bytes), Err(Error::Index)));
        let mut bytes = fixture();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let a = Animation::parse(&bytes).unwrap();
        let mut work = vec![0; WORK_BYTES];
        assert!(a.decode(1, &mut work).is_err());
        assert_eq!(a.decode(0, &mut work[..99]), Err(Error::WorkBuffer));
    }
}
