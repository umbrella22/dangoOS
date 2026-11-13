//! RCR1 USB framing. Payload limits bound one transfer, never an export.
//! Every request is independent: its ID and absolute address survive reconnects.
use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"RCR1";
pub const HEADER: usize = 16;
pub const PAYLOAD: usize = 4096;
pub const FRAME: usize = HEADER + PAYLOAD;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Hello = 1,
    Nor = 2,
    NandPage = 3,
    Mount = 4,
    List = 5,
    File = 6,
    Logical = 7,
    Status = 8,
}
impl TryFrom<u8> for Operation {
    type Error = Error;
    fn try_from(v: u8) -> Result<Self, Error> {
        Ok(match v {
            1 => Self::Hello,
            2 => Self::Nor,
            3 => Self::NandPage,
            4 => Self::Mount,
            5 => Self::List,
            6 => Self::File,
            7 => Self::Logical,
            8 => Self::Status,
            _ => return Err(Error::Operation),
        })
    }
}
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok = 0,
    Invalid = 1,
    Io = 2,
    Unavailable = 3,
    ReadOnly = 4,
    NotFound = 5,
    Cancelled = 6,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Length,
    Crc,
    Operation,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub id: u32,
    pub operation: u8,
    pub status: u8,
    pub data: Vec<u8>,
}
pub fn crc32(bytes: &[u8]) -> u32 {
    frame_crc(bytes, &[])
}
fn frame_crc(header: &[u8], payload: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut bit = 0;
            while bit < 8 {
                c = (c >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(c & 1));
                bit += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    let mut crc = !0u32;
    for &byte in header.iter().chain(payload) {
        crc = (crc >> 8) ^ TABLE[(crc as u8 ^ byte) as usize];
    }
    !crc
}
impl Packet {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.data.len() > PAYLOAD {
            return Err(Error::Length);
        }
        let mut out = Vec::with_capacity(HEADER + self.data.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&self.id.to_le_bytes());
        out.push(self.operation);
        out.push(self.status);
        out.extend_from_slice(&(self.data.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&self.data);
        let crc = frame_crc(&out[4..12], &self.data);
        out[12..16].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
}
/// Incremental decoder with a fixed capacity. Noise and malformed headers do
/// not accumulate; a new magic word reestablishes framing after USB reconnect.
pub struct Decoder {
    buf: Vec<u8>,
}
impl Default for Decoder {
    fn default() -> Self {
        Self {
            buf: Vec::with_capacity(FRAME),
        }
    }
}
impl Decoder {
    pub fn reset(&mut self) {
        self.buf.clear();
    }
    pub fn push(&mut self, byte: u8) -> Option<Result<Packet, Error>> {
        self.buf.push(byte);
        if self.buf.len() <= 4 {
            while !MAGIC.starts_with(&self.buf) {
                self.buf.remove(0);
            }
            return None;
        }
        if self.buf.len() < HEADER {
            return None;
        }
        let len = u16::from_le_bytes(self.buf[10..12].try_into().unwrap()) as usize;
        if len > PAYLOAD {
            self.reset();
            return Some(Err(Error::Length));
        }
        if self.buf.len() < HEADER + len {
            return None;
        }
        let expected = u32::from_le_bytes(self.buf[12..16].try_into().unwrap());
        let actual = frame_crc(&self.buf[4..12], &self.buf[HEADER..]);
        let result = if expected == actual {
            Ok(Packet {
                id: u32::from_le_bytes(self.buf[4..8].try_into().unwrap()),
                operation: self.buf[8],
                status: self.buf[9],
                data: self.buf[HEADER..].into(),
            })
        } else {
            Err(Error::Crc)
        };
        self.reset();
        Some(result)
    }
}

/// A byte range on a specific medium. The caller supplies the actual capacity.
pub fn read_range(offset: u64, length: usize, capacity: u64) -> Result<(), Error> {
    if length > PAYLOAD
        || offset
            .checked_add(length as u64)
            .is_none_or(|end| end > capacity)
    {
        return Err(Error::Length);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    #[test]
    fn crc_matches_ieee_usb_and_asset_contract() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(frame_crc(b"1234", b"56789"), 0xcbf4_3926);
        assert_eq!(crc32(&[]), 0);
    }
    #[test]
    fn binary_payload_and_arbitrary_usb_boundaries_roundtrip() {
        let p = Packet {
            id: 0xf000_1234,
            operation: 3,
            status: 0,
            data: (0..PAYLOAD).map(|i| i as u8).collect(),
        };
        let wire = p.encode().unwrap();
        for chunk in [1, 7, 64, 512] {
            let mut decoder = Decoder::default();
            let mut got = vec![];
            for part in wire.chunks(chunk) {
                for &byte in part {
                    if let Some(p) = decoder.push(byte) {
                        got.push(p);
                    }
                }
            }
            assert_eq!(got, vec![Ok(p.clone())]);
        }
    }
    #[test]
    fn damaged_payload_header_and_noise_cannot_be_accepted_as_data() {
        let p = Packet {
            id: 19,
            operation: 2,
            status: 0,
            data: b"RCR1\0\xff".into(),
        };
        for pos in [4, 8, 9, 12, 16, 20] {
            let mut wire = p.encode().unwrap();
            wire[pos] ^= 0x80;
            let mut d = Decoder::default();
            let result: Vec<_> = wire.into_iter().filter_map(|b| d.push(b)).collect();
            assert_eq!(result, vec![Err(Error::Crc)]);
            assert_eq!(
                p.encode().unwrap().into_iter().find_map(|b| d.push(b)),
                Some(Ok(p.clone()))
            );
        }
        let mut d = Decoder::default();
        for b in b"\0junkRRCRbroken" {
            let _ = d.push(*b);
        }
        assert_eq!(
            p.encode().unwrap().into_iter().find_map(|b| d.push(b)),
            Some(Ok(p))
        );
    }
    #[test]
    fn transfer_limit_does_not_limit_total_medium_or_resume_offset() {
        assert_eq!(
            read_range(128 * 1024 * 1024, 2176, 256 * 1024 * 1024),
            Ok(())
        );
        assert_eq!(read_range(u64::MAX, 1, u64::MAX), Err(Error::Length));
        assert_eq!(read_range(100, 1, 100), Err(Error::Length));
    }
}
