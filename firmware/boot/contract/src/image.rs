//! Streaming S31ONE03 main-image verification without a whole-image allocation.
use sha2::{Digest, Sha256};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.0)
    }
}
pub type Result<T> = core::result::Result<T, Error>;
pub const MAIN_MARKER: &[u8] = b"DANGO_S31_SINGLE_MAIN_S31ONE03\0";
#[derive(Clone, Copy)]
enum Phase {
    Header,
    SegmentHeader,
    Data,
    Padding,
    Hash,
    Done,
}
pub struct ImageVerifier {
    phase: Phase,
    offset: u32,
    total: u32,
    small: [u8; 32],
    used: usize,
    segments: u8,
    remaining: u32,
    checksum: u8,
    hash: Sha256,
    expected: [u8; 32],
    ranges: [(u32, u32); 16],
    count: usize,
    entry: u32,
    appdesc: bool,
    descriptor: [u8; 256],
    descriptor_used: usize,
    marker: usize,
    single_main: bool,
}
fn word(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}
fn memory(addr: u32, len: u32) -> bool {
    addr.checked_add(len).is_some_and(|end| {
        [
            (0x40000000, 0x54000000),
            (0x2f000000, 0x2f080000),
            (0x2e000000, 0x2e008000),
        ]
        .iter()
        .any(|&(lo, hi)| addr >= lo && end <= hi)
    })
}
impl ImageVerifier {
    pub fn new(total: u32) -> Self {
        Self {
            phase: Phase::Header,
            offset: 0,
            total,
            small: [0; 32],
            used: 0,
            segments: 0,
            remaining: 0,
            checksum: 0xef,
            hash: Sha256::new(),
            expected: [0; 32],
            ranges: [(0, 0); 16],
            count: 0,
            entry: 0,
            appdesc: false,
            descriptor: [0; 256],
            descriptor_used: 0,
            marker: 0,
            single_main: false,
        }
    }
    pub fn feed(&mut self, data: &[u8]) -> Result<()> {
        for &byte in data {
            let marker = MAIN_MARKER;
            if matches!(self.phase, Phase::Hash | Phase::Done) {
                self.marker = 0;
            } else if byte == marker[self.marker] {
                self.marker += 1;
                if self.marker == marker.len() {
                    self.single_main = true;
                    self.marker = 0;
                }
            } else {
                self.marker = usize::from(byte == marker[0]);
            }
            if self.offset >= self.total {
                return Err(Error("trailing payload"));
            }
            if !matches!(self.phase, Phase::Hash | Phase::Done) {
                self.hash.update([byte]);
            }
            self.offset += 1;
            match self.phase {
                Phase::Header => {
                    self.small[self.used] = byte;
                    self.used += 1;
                    if self.used == 24 {
                        let h = &self.small;
                        if h[0] != 0xe9 || !(1..=16).contains(&h[1]) {
                            return Err(Error("header/segment count"));
                        }
                        if u16::from_le_bytes([h[12], h[13]]) != 32 {
                            return Err(Error("chip is not S31"));
                        }
                        if h[2] != 2 || h[3] >> 4 != 4 || h[3] & 15 != 0 {
                            return Err(Error("requires 16MiB/DIO/40MHz image"));
                        }
                        if h[23] != 1 || h[19..23] != [0; 4] {
                            return Err(Error("hash flag/reserved fields"));
                        }
                        self.entry = word(&h[4..8]);
                        if !memory(self.entry, 1) {
                            return Err(Error("entry point"));
                        }
                        self.segments = h[1];
                        self.used = 0;
                        self.phase = Phase::SegmentHeader;
                    }
                }
                Phase::SegmentHeader => {
                    self.small[self.used] = byte;
                    self.used += 1;
                    if self.used == 8 {
                        let addr = word(&self.small);
                        let len = word(&self.small[4..]);
                        if len == 0
                            || self.count == 0 && len < 256
                            || len % 4 != 0
                            || !memory(addr, len)
                            || len > self.total.saturating_sub(self.offset)
                        {
                            return Err(Error("segment range/length"));
                        }
                        let end = addr.checked_add(len).ok_or(Error("segment overflow"))?;
                        if self.ranges[..self.count]
                            .iter()
                            .any(|&(a, b)| addr < b && a < end)
                        {
                            return Err(Error("overlapping load segments"));
                        }
                        self.ranges[self.count] = (addr, end);
                        self.count += 1;
                        self.remaining = len;
                        self.used = 0;
                        self.phase = Phase::Data;
                    }
                }
                Phase::Data => {
                    self.checksum ^= byte;
                    if self.count == 1 && self.descriptor_used < 256 {
                        self.descriptor[self.descriptor_used] = byte;
                        self.descriptor_used += 1;
                        if self.descriptor_used == 256 {
                            let d = &self.descriptor;
                            self.appdesc = word(d) == 0xabcd5432;
                            if !self.appdesc {
                                return Err(Error("missing app descriptor"));
                            }
                            let project = &d[48..80];
                            if !project.starts_with(b"mosaico-s31\0")
                                || project[11..].iter().any(|&b| b != 0)
                            {
                                return Err(Error("app descriptor project"));
                            }
                            let version = &d[16..48];
                            let Some(n) = version.iter().position(|&b| b == 0) else {
                                return Err(Error("unterminated app version"));
                            };
                            if n == 0
                                || !version[..n].iter().all(|b| b.is_ascii_graphic())
                                || version[n..].iter().any(|&b| b != 0)
                            {
                                return Err(Error("invalid app version"));
                            }
                        }
                    }
                    self.remaining -= 1;
                    if self.remaining == 0 {
                        self.segments -= 1;
                        self.used = 0;
                        self.phase = if self.segments == 0 {
                            Phase::Padding
                        } else {
                            Phase::SegmentHeader
                        };
                    }
                }
                Phase::Padding => {
                    if self.offset % 16 == 0 {
                        if byte != self.checksum {
                            return Err(Error("XOR checksum"));
                        }
                        self.expected = self.hash.clone().finalize().into();
                        self.used = 0;
                        self.phase = Phase::Hash;
                    } else if byte != 0 {
                        return Err(Error("nonzero padding"));
                    }
                }
                Phase::Hash => {
                    self.small[self.used] = byte;
                    self.used += 1;
                    if self.used == 32 {
                        if self.small != self.expected {
                            return Err(Error("embedded SHA256"));
                        }
                        self.phase = Phase::Done;
                    }
                }
                Phase::Done => return Err(Error("trailing payload")),
            }
        }
        Ok(())
    }
    pub fn version(&self) -> Option<&str> {
        if !self.appdesc {
            return None;
        }
        let version = &self.descriptor[16..48];
        let end = version.iter().position(|b| *b == 0)?;
        core::str::from_utf8(&version[..end]).ok()
    }
    pub fn single_main(&self) -> bool {
        self.single_main
    }
    pub fn finish(&self) -> Result<()> {
        if !matches!(self.phase, Phase::Done)
            || self.offset != self.total
            || !self.appdesc
            || self.total > crate::MAIN_BYTES
        {
            return Err(Error("truncated image"));
        }
        if !self.ranges[..self.count]
            .iter()
            .any(|&(a, b)| self.entry >= a && self.entry < b)
        {
            return Err(Error("entry outside loaded segments"));
        }
        if !self.single_main {
            return Err(Error("missing S31ONE03 main contract"));
        }
        Ok(())
    }
}
