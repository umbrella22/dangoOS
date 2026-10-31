//! Host-testable A/B updater. Every operation is bounded to one flash sector.
use alloc::{format, string::String};
use core::fmt;
use sha2::{Digest, Sha256};
pub mod image;
pub mod layout;
pub mod status;
pub const AB_LAYOUT_MARKER: &[u8] = b"MOSAICO_S31_AB_LAYOUT_V2_6M_NAND\0";
pub const SLOT_SIZE: u32 = 0x600000;
pub const SECTOR: usize = 4096;
pub type Result<T> = core::result::Result<T, Error>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Busy,
    Flash,
    Layout(&'static str),
    Image(&'static str),
    State(&'static str),
    Transport(&'static str),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("A/B unavailable"),
            Self::Busy => f.write_str("OTA busy"),
            Self::Flash => f.write_str("flash IO/readback failure"),
            Self::Layout(s) => write!(f, "layout: {s}"),
            Self::Image(s) => write!(f, "image: {s}"),
            Self::State(s) => write!(f, "state: {s}"),
            Self::Transport(s) => write!(f, "transport: {s}"),
        }
    }
}
pub trait Flash {
    fn read(&mut self, address: u32, out: &mut [u8]) -> Result<()>;
    fn write(&mut self, address: u32, data: &[u8]) -> Result<()>;
    fn erase(&mut self, address: u32) -> Result<()>;
}
pub fn slot_offset(slot: u8) -> Result<u32> {
    match slot {
        0 => Ok(0x210000),
        1 => Ok(0x810000),
        _ => Err(Error::State("unknown slot")),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub seq: u32,
    pub state: u32,
}
/// ESP-IDF crc32_le(UINT32_MAX, seq, 4), matching pinned IDF wrapper.
pub fn entry_crc(seq: u32) -> u32 {
    let mut crc = 0u32;
    for byte in seq.to_le_bytes() {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
        }
    }
    !crc
}
impl Entry {
    pub fn parse(bytes: &[u8]) -> Result<Option<Self>> {
        if bytes.len() != 32 {
            return Err(Error::State("otadata entry size"));
        }
        if bytes.iter().all(|&b| b == 255) {
            return Ok(None);
        }
        let word = |n| u32::from_le_bytes(bytes[n..n + 4].try_into().unwrap());
        let (seq, state) = (word(0), word(24));
        if seq == 0
            || seq == u32::MAX
            || ![0, 1, 2, 3, 4, u32::MAX].contains(&state)
            || word(28) != entry_crc(seq)
        {
            return Err(Error::State("otadata CRC/state/sequence"));
        }
        Ok(Some(Self { seq, state }))
    }
    pub fn bytes(self) -> [u8; 32] {
        let mut b = [255; 32];
        b[..4].copy_from_slice(&self.seq.to_le_bytes());
        b[24..28].copy_from_slice(&self.state.to_le_bytes());
        b[28..].copy_from_slice(&entry_crc(self.seq).to_le_bytes());
        b
    }
    pub fn slot(self) -> u8 {
        ((self.seq - 1) % 2) as u8
    }
    pub fn bootable(self) -> bool {
        !matches!(self.state, 3 | 4)
    }
}
pub fn selected(entries: [Option<Entry>; 2]) -> Result<Option<(usize, Entry)>> {
    if entries[0]
        .zip(entries[1])
        .is_some_and(|(a, b)| a.seq == b.seq)
    {
        return Err(Error::State("ambiguous equal OTA sequences"));
    }
    Ok(entries
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| e.filter(|e| e.bootable()).map(|e| (i, e)))
        .max_by_key(|(_, e)| e.seq))
}
pub fn read_entries(flash: &mut impl Flash) -> Result<[Option<Entry>; 2]> {
    let mut result = [None; 2];
    let mut bad = 0;
    let mut erased = 0;
    for (i, e) in result.iter_mut().enumerate() {
        let mut bytes = [0; 32];
        flash.read(0x9000 + i as u32 * 4096, &mut bytes)?;
        match Entry::parse(&bytes) {
            Ok(Some(entry)) => *e = Some(entry),
            Ok(None) => erased += 1,
            Err(_) => bad += 1,
        }
    }
    if bad > 0 && result.iter().all(Option::is_none) {
        return Err(Error::State("no valid otadata copy"));
    }
    if erased == 2 {
        return Ok([None; 2]);
    }
    Ok(result)
}

fn persist_entry(flash: &mut impl Flash, index: usize, entry: Entry) -> Result<()> {
    let address = 0x9000 + index as u32 * 4096;
    flash.erase(address)?;
    let bytes = entry.bytes();
    flash.write(address, &bytes)?;
    let mut read = [0; 32];
    flash.read(address, &mut read)?;
    if read != bytes {
        return Err(Error::Flash);
    }
    Ok(())
}
/// Updates the alternate metadata sector, preserving the current bootable copy.
pub fn select_slot(flash: &mut impl Flash, booted: u8, target: u8, state: u32) -> Result<Entry> {
    slot_offset(target)?;
    slot_offset(booted)?;
    if ![0, 1, 2].contains(&state) {
        return Err(Error::State("invalid selection state"));
    }
    let mut entries = read_entries(flash)?;
    let mut current = selected(entries)?;
    if current.is_none() && entries.iter().any(Option::is_some) {
        return Err(Error::State("no bootable OTA selection"));
    }
    if current.is_none() && booted == 0 {
        let baseline = Entry { seq: 1, state: 2 };
        persist_entry(flash, 0, baseline)?;
        entries[0] = Some(baseline);
        current = Some((0, baseline));
    }
    if current.is_some_and(|(_, e)| e.slot() != booted) {
        return Err(Error::State("selected/booted mismatch"));
    }
    let index = current.map_or(0, |(i, _)| 1 - i);
    let max = entries.iter().flatten().map(|e| e.seq).max().unwrap_or(0);
    let mut seq = max
        .checked_add(1)
        .ok_or(Error::State("OTA sequence exhausted"))?;
    if (seq - 1) % 2 != target as u32 {
        seq = seq
            .checked_add(1)
            .ok_or(Error::State("OTA sequence exhausted"))?;
    }
    if seq == u32::MAX {
        return Err(Error::State("OTA sequence exhausted"));
    }
    let entry = Entry { seq, state };
    persist_entry(flash, index, entry)?;
    Ok(entry)
}
/// Confirms only the selected entry corresponding to the actual MMU booted slot.
pub fn confirm(flash: &mut impl Flash, booted: u8) -> Result<()> {
    let entries = read_entries(flash)?;
    let Some((index, entry)) = selected(entries)? else {
        if booted != 0 {
            return Err(Error::State("blank metadata requires slot zero"));
        }
        return persist_entry(flash, 0, Entry { seq: 1, state: 2 });
    };
    if entry.slot() != booted {
        return Err(Error::State("selected/booted mismatch"));
    }
    if entry.state == 2 {
        return Ok(());
    }
    persist_entry(flash, index, Entry { state: 2, ..entry })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Receiving,
    Verifying,
    Activated,
    Failed,
}
struct Transfer {
    size: u32,
    written: u32,
    expected: [u8; 32],
    transport: Sha256,
    readback: Sha256,
    verifier: image::ImageVerifier,
    checked: u32,
}
pub struct Updater {
    confirmed: bool,
    booted: u8,
    target: u8,
    pub phase: Phase,
    transfer: Option<Transfer>,
}
impl Updater {
    pub fn booted(&self) -> u8 {
        self.booted
    }
    pub fn target(&self) -> u8 {
        self.target
    }
    pub fn new(layout: &layout::Layout, booted: u8, flash: &mut impl Flash) -> Result<Self> {
        if !layout.ab {
            return Err(Error::Unavailable);
        }
        slot_offset(booted)?;
        let entries = read_entries(flash)?;
        let current = selected(entries)?;
        if current.is_some_and(|(_, e)| e.slot() != booted) || current.is_none() && booted != 0 {
            return Err(Error::State("selected/booted mismatch"));
        }
        Ok(Self {
            confirmed: current.is_some_and(|(_, e)| e.state == 2),
            booted,
            target: 1 - booted,
            phase: Phase::Idle,
            transfer: None,
        })
    }
    pub fn begin(&mut self, size: u32, sha: [u8; 32]) -> Result<()> {
        if !self.confirmed {
            return Err(Error::State("running slot is not confirmed Valid"));
        }
        if matches!(
            self.phase,
            Phase::Receiving | Phase::Verifying | Phase::Activated
        ) {
            return Err(Error::Busy);
        }
        if size < 64 || size > SLOT_SIZE {
            return Err(Error::Transport("image size outside slot"));
        }
        self.transfer = Some(Transfer {
            size,
            written: 0,
            expected: sha,
            transport: Sha256::new(),
            readback: Sha256::new(),
            verifier: image::ImageVerifier::new(size),
            checked: 0,
        });
        self.phase = Phase::Receiving;
        Ok(())
    }
    pub fn chunk(
        &mut self,
        flash: &mut impl Flash,
        offset: u32,
        bytes: &[u8],
        crc: u32,
    ) -> Result<u32> {
        let r = self.chunk_inner(flash, offset, bytes, crc);
        if r.is_err() {
            self.abort();
            self.phase = Phase::Failed;
        }
        r
    }
    fn chunk_inner(
        &mut self,
        flash: &mut impl Flash,
        offset: u32,
        bytes: &[u8],
        crc: u32,
    ) -> Result<u32> {
        if self.booted == self.target {
            return Err(Error::State("running slot target"));
        }
        if self.phase != Phase::Receiving {
            return Err(Error::State("not receiving"));
        }
        let t = self
            .transfer
            .as_mut()
            .ok_or(Error::State("missing transfer"))?;
        let left = t.size - t.written;
        if offset != t.written
            || offset % SECTOR as u32 != 0
            || bytes.len() != left.min(SECTOR as u32) as usize
            || crate::formats::crc32::crc32(bytes) != crc
        {
            return Err(Error::Transport("chunk offset/length/CRC"));
        }
        let address = slot_offset(self.target)? + offset;
        let mut sector = [255; SECTOR];
        sector[..bytes.len()].copy_from_slice(bytes);
        flash.erase(address)?;
        let aligned = (bytes.len() + 3) & !3;
        flash.write(address, &sector[..aligned])?;
        let mut check = [0; SECTOR];
        flash.read(address, &mut check[..aligned])?;
        if check[..aligned] != sector[..aligned] {
            return Err(Error::Flash);
        }
        t.transport.update(bytes);
        t.written += bytes.len() as u32;
        Ok(t.written)
    }
    pub fn start_verify(&mut self) -> Result<()> {
        if self.phase != Phase::Receiving {
            return Err(Error::State("not receiving"));
        }
        let t = self.transfer.as_ref().unwrap();
        if t.written != t.size || t.transport.clone().finalize()[..] != t.expected {
            return Err(Error::Transport("incomplete/transport SHA256"));
        }
        self.phase = Phase::Verifying;
        Ok(())
    }
    /// One sector readback and validation step. Caller yields between false results.
    pub fn verify_step(&mut self, flash: &mut impl Flash) -> Result<bool> {
        let r = self.verify_inner(flash);
        if r.is_err() {
            self.abort();
            self.phase = Phase::Failed;
        }
        r
    }
    fn verify_inner(&mut self, flash: &mut impl Flash) -> Result<bool> {
        if self.phase != Phase::Verifying {
            return Err(Error::State("not verifying"));
        }
        let t = self.transfer.as_mut().unwrap();
        let n = (t.size - t.checked).min(SECTOR as u32) as usize;
        let mut bytes = [0; SECTOR];
        flash.read(
            slot_offset(self.target)? + t.checked,
            &mut bytes[..(n + 3) & !3],
        )?;
        t.readback.update(&bytes[..n]);
        t.verifier.feed(&bytes[..n])?;
        t.checked += n as u32;
        if t.checked < t.size {
            return Ok(false);
        }
        if t.readback.clone().finalize()[..] != t.expected {
            return Err(Error::Transport("readback SHA256"));
        }
        t.verifier.finish()?;
        if self.booted == self.target {
            return Err(Error::State("running slot target"));
        }
        if !t.verifier.ab_layout() {
            return Err(Error::Image("missing A/B layout compatibility marker"));
        }
        select_slot(flash, self.booted, self.target, 0)?;
        self.phase = Phase::Activated;
        Ok(true)
    }
    pub fn abort(&mut self) {
        self.transfer = None;
        self.phase = Phase::Idle;
    }
    pub fn status(&self) -> String {
        let (written, total) = self
            .transfer
            .as_ref()
            .map_or((0, 0), |t| (t.written, t.size));
        format!(
            "phase={:?} booted={} target={} received={written} total={total}",
            self.phase, self.booted, self.target
        )
    }
}
pub fn parse_sha(text: &str) -> Result<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(Error::Transport("SHA256 must contain 64 ASCII hex digits"));
    }
    let nibble = |b: u8| {
        if b.is_ascii_digit() {
            b - b'0'
        } else {
            b.to_ascii_lowercase() - b'a' + 10
        }
    };
    let mut out = [0; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = nibble(bytes[2 * i]) * 16 + nibble(bytes[2 * i + 1]);
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Begin {
        size: u32,
        sha: [u8; 32],
    },
    Chunk {
        offset: u32,
        bytes: alloc::vec::Vec<u8>,
        crc: u32,
    },
    End,
    Abort,
}
impl Command {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Status => "ota-status",
            Self::Begin { .. } => "ota-begin",
            Self::Chunk { .. } => "ota-chunk",
            Self::End => "ota-end",
            Self::Abort => "ota-abort",
        }
    }
}
#[cfg(test)]
mod tests;

/// Continuous healthy UI-loop window. A gap over one second restarts probation.
#[derive(Default)]
pub struct HealthWindow {
    last: Option<u64>,
    since: Option<u64>,
}
impl HealthWindow {
    pub const fn new() -> Self {
        Self {
            last: None,
            since: None,
        }
    }
    pub fn observe(&mut self, now_ms: u64, healthy: bool) -> bool {
        if !healthy {
            self.last = None;
            self.since = None;
            return false;
        }
        if self
            .last
            .is_none_or(|last| now_ms < last || now_ms - last > 1000)
        {
            self.since = Some(now_ms);
        }
        self.last = Some(now_ms);
        now_ms.saturating_sub(self.since.unwrap_or(now_ms)) >= 30_000
    }
}

/// Two failed candidate boots are followed by fallback at the third main entry.
pub fn warm_fallback_due(attempts: u32) -> bool {
    attempts >= 3
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebootAck {
    Waiting,
    Sent,
    TimedOut,
}
/// Successful endpoint completion permits reset; a three-second deadline never does.
pub fn reboot_ack(sent: bool, elapsed_ms: u64) -> RebootAck {
    if sent {
        RebootAck::Sent
    } else if elapsed_ms >= 3000 {
        RebootAck::TimedOut
    } else {
        RebootAck::Waiting
    }
}

#[cfg(target_os = "none")]
pub(crate) mod device;
