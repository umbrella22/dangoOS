//! GD5F1GM7UE commands. Writes require explicit prepare_writes().
//!
//! Framing follows Espressif spi_nand_flash's single-line discovery: 9Fh,
//! one zero address/dummy byte, ID bytes; 0Fh, feature address, one result byte. Raw IDs
//! are preserved. Page access requires the exact verified model ID and normal
//! array mode with ECC enabled. Source: GigaDevice DS-SP00859 Rev1.6MT, sections
//! 8.2, 8.3, 8.9, 12.1, 12.2 and 12.4 (September 2024).

pub const DATA_BYTES: usize = 2048;
pub const SPARE_BYTES: usize = 128;
pub const PAGE_BYTES: usize = DATA_BYTES + SPARE_BYTES;
pub const PAGES_PER_BLOCK: u32 = 64;
pub const BLOCKS: u32 = 1024;
pub const PAGE_COUNT: u32 = PAGES_PER_BLOCK * BLOCKS;
// Datasheet tRD_ECC max is 120 us. Allow 2 ms for the command/status path.
const READ_TIMEOUT_US: u64 = 2000;
// Datasheet maxima: program 600 us, erase 10 ms. Include bus/poll margin.
const PROGRAM_TIMEOUT_US: u64 = 5_000;
const ERASE_TIMEOUT_US: u64 = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Feature(u8),
    Enable,
    Load(u16),
    Execute(u16),
    Erase(u16),
}
impl Write {
    #[cfg(test)]
    pub const fn frame(self) -> (u8, u32, u8) {
        self.frame_for(false)
    }
    /// QOUT uses a single-line command/address and four data lines.
    pub const fn frame_for(self, quad: bool) -> (u8, u32, u8) {
        match self {
            Self::Feature(a) => (0x1f, a as u32, 8),
            Self::Enable => (0x06, 0, 0),
            Self::Load(a) => (if quad { 0x32 } else { 0x02 }, a as u32, 16),
            Self::Execute(p) => (0x10, p as u32, 24),
            Self::Erase(p) => (0xd8, p as u32, 24),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    JedecId,
    Feature(u8),
    Cache(u16),
}
/// SPI wire phases. Address bits and dummy clocks belong to the NAND protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadFrame {
    pub opcode: u8,
    pub address: u16,
    pub address_bits: u8,
    pub dummy_cycles: u8,
}
impl Read {
    #[cfg(test)]
    pub const fn frame(self) -> ReadFrame {
        self.frame_for(false)
    }
    pub const fn frame_for(self, quad: bool) -> ReadFrame {
        let (opcode, address, address_bits, dummy_cycles) = match self {
            // Explicit MOSI=0 after Read ID, rather than a floating dummy byte.
            Self::JedecId => (0x9f, 0, 8, 0),
            Self::Feature(address) => (0x0f, address as u16, 8, 0),
            Self::Cache(column) => (if quad { 0x6b } else { 0x03 }, column, 16, 8),
        };
        ReadFrame {
            opcode,
            address,
            address_bits,
            dummy_cycles,
        }
    }
}

/// One chip-select assertion per command. The controller owns CS and idle-high.
pub trait Transport {
    type Error;
    fn read(&mut self, command: Read, bytes: &mut [u8]) -> Result<(), Self::Error>;
    fn write(&mut self, command: Write, bytes: &[u8]) -> Result<(), Self::Error>;
    /// 13h + 24-bit row, no payload: transfers array data into volatile cache.
    fn load_page(&mut self, page: u16) -> Result<(), Self::Error>;
    fn now_us(&self) -> u64;
    fn delay_us(&mut self, us: u32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub id: [u8; 4],
    pub protection: u8,
    pub configuration: u8,
    pub status: u8,
}

impl Identity {
    pub fn model(&self) -> Option<&'static str> {
        // This part repeats its two-byte ID. Do not accept a shared prefix:
        // GD5F1GM9UE reports C8 91 01 and has different extended ECC semantics.
        (self.id == [0xc8, 0x91, 0xc8, 0x91]).then_some("GD5F1GM7UE")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageStatus {
    pub status: u8,
    pub extended_status: Option<u8>,
    /// Corrected bit count per 512-byte step, including the documented range.
    pub corrected_min: u8,
    pub corrected_max: u8,
}

fn decode_ecc<E>(status: u8, extended: Option<u8>) -> Result<PageStatus, Error<E>> {
    let (min, max) = match (status >> 4) & 3 {
        0 => (0, 0),
        1 => match (extended.expect("extended status required for ECCS=01") >> 4) & 3 {
            0 => (1, 4),
            n => (4 + n, 4 + n),
        },
        2 => return Err(Error::Uncorrectable),
        3 => (8, 8),
        _ => unreachable!(),
    };
    Ok(PageStatus {
        status,
        extended_status: extended,
        corrected_min: min,
        corrected_max: max,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error<E> {
    Transport(E),
    NoResponse,
    UnstableId,
    Busy,
    UnknownModel,
    PageRange,
    BufferSize,
    OtpEnabled,
    EccDisabled,
    ReadTimeout,
    Uncorrectable,
    WritesDisabled,
    BlockRange,
    BadBlock,
    Protected,
    WriteEnable,
    FeatureVerify,
    ConfigurationRestore,
    ProgramTimeout,
    EraseTimeout,
    ProgramFailed,
    EraseFailed,
    MarkerFailed,
}

pub struct Nand<T> {
    transport: T,
    writable: bool,
    // Immutable soldered-chip identity only. Status/protection/configuration
    // are read live before every operation, including after a cache hit here.
    verified_id: Option<[u8; 4]>,
    quad: bool,
    // Scanned before any mutation, retained for the lifetime of this device.
    bad: Option<[u8; BLOCKS as usize / 8]>,
}

impl<T: Transport> Nand<T> {
    pub const fn new(transport: T) -> Self {
        Self {
            transport,
            writable: false,
            verified_id: None,
            quad: false,
            bad: None,
        }
    }

    /// Transfer unique ownership from bootstrap PIO to the DMA adapter.
    pub fn map_transport<U: Transport>(self, f: impl FnOnce(T) -> U) -> Nand<U> {
        Nand {
            transport: f(self.transport),
            writable: self.writable,
            verified_id: self.verified_id,
            quad: self.quad,
            bad: self.bad,
        }
    }

    /// Enable only this verified model's QE bit before mapping HOLD#/WP# to
    /// SIO3/SIO2. No array, protection, OTP or ECC setting is changed.
    pub fn enable_quad(&mut self) -> Result<(), Error<T::Error>> {
        let id = self.normal()?;
        if id.configuration & 1 == 0 {
            self.set_feature(0xb0, id.configuration | 1)?;
        }
        self.quad = true;
        Ok(())
    }

    fn write(&mut self, command: Write, bytes: &[u8]) -> Result<(), Error<T::Error>> {
        self.transport
            .write(command, bytes)
            .map_err(Error::Transport)
    }
    fn set_feature(&mut self, address: u8, value: u8) -> Result<(), Error<T::Error>> {
        self.write(Write::Feature(address), &[value])?;
        if self.feature(address)? != value {
            return Err(Error::FeatureVerify);
        }
        Ok(())
    }
    fn normal(&mut self) -> Result<Identity, Error<T::Error>> {
        let id = if let Some(id) = self.verified_id {
            let status = self.feature(0xc0)?;
            if status & 1 != 0 {
                return Err(Error::Busy);
            }
            Identity {
                id,
                status,
                protection: self.feature(0xa0)?,
                configuration: self.feature(0xb0)?,
            }
        } else {
            self.identify()?
        };
        if id.model().is_none() {
            return Err(Error::UnknownModel);
        }
        if id.configuration & 0x40 != 0 {
            return Err(Error::OtpEnabled);
        }
        if id.configuration & 0x10 == 0 {
            return Err(Error::EccDisabled);
        }
        if self.quad && id.configuration & 1 == 0 {
            return Err(Error::FeatureVerify);
        }
        Ok(id)
    }
    fn wait(&mut self, timeout: u64, error: Error<T::Error>) -> Result<u8, Error<T::Error>> {
        let start = self.transport.now_us();
        loop {
            // A BUSY sample can become stale while radio/RTOS work preempts us.
            // Only time out on BUSY sampled after the deadline; otherwise poll
            // again so a completed NAND operation is not reported as failed.
            let expired = self.transport.now_us().saturating_sub(start) >= timeout;
            let status = self.feature(0xc0)?;
            if status & 1 == 0 {
                return Ok(status);
            }
            if expired {
                return Err(error);
            }
            self.transport.delay_us(20);
        }
    }
    /// Unlock only normal array access. Never changes OTP, BPL or QE.
    pub fn prepare_writes(&mut self) -> Result<(), Error<T::Error>> {
        self.writable = false;
        let id = self.normal()?;
        if id.configuration & 8 != 0 {
            return Err(Error::Protected);
        }
        self.scan_bad_blocks()?;
        self.set_feature(0xa0, id.protection & 0x80)?;
        self.writable = true;
        Ok(())
    }
    fn write_enable(&mut self) -> Result<(), Error<T::Error>> {
        if !self.writable {
            return Err(Error::WritesDisabled);
        }
        let id = self.normal()?;
        if id.protection & 0x7e != 0 {
            return Err(Error::Protected);
        }
        self.write(Write::Enable, &[])?;
        if self.feature(0xc0)? & 3 != 2 {
            return Err(Error::WriteEnable);
        }
        Ok(())
    }
    /// Raw inspection with ECC disabled; always restores and verifies the mode.
    /// Factory markers and erased-page detection must not use corrected bytes.
    fn raw_read(
        &mut self,
        page: u32,
        column: u16,
        output: &mut [u8],
    ) -> Result<(), Error<T::Error>> {
        if page >= PAGE_COUNT {
            return Err(Error::PageRange);
        }
        if column as usize + output.len() > PAGE_BYTES {
            return Err(Error::BufferSize);
        }
        let id = self.normal()?;
        let read = (|| {
            self.set_feature(0xb0, id.configuration & !0x10)?;
            self.transport
                .load_page(page as u16)
                .map_err(Error::Transport)?;
            self.wait(READ_TIMEOUT_US, Error::ReadTimeout)?;
            self.transport
                .read(Read::Cache(column), output)
                .map_err(Error::Transport)
        })();
        if self.set_feature(0xb0, id.configuration).is_err() {
            self.writable = false;
            return Err(Error::ConfigurationRestore);
        }
        read
    }
    pub fn bad_marker(&mut self, block: u32) -> Result<u8, Error<T::Error>> {
        if block >= BLOCKS {
            return Err(Error::BlockRange);
        }
        let mut marker = [0];
        self.raw_read(block * PAGES_PER_BLOCK, DATA_BYTES as u16, &mut marker)?;
        Ok(marker[0])
    }
    /// Read every factory/runtime marker before any program or erase. An
    /// interrupted scan never replaces the previous complete table.
    pub fn scan_bad_blocks(&mut self) -> Result<u32, Error<T::Error>> {
        let mut table = [0u8; BLOCKS as usize / 8];
        let mut count = 0;
        for block in 0..BLOCKS {
            if self.bad_marker(block)? != 0xff {
                table[block as usize / 8] |= 1 << (block % 8);
                count += 1;
            }
        }
        self.bad = Some(table);
        Ok(count)
    }
    pub fn bad_count(&self) -> Option<u32> {
        self.bad
            .as_ref()
            .map(|table| table.iter().map(|v| v.count_ones()).sum())
    }
    pub fn is_bad(&mut self, block: u32) -> Result<bool, Error<T::Error>> {
        if block >= BLOCKS {
            return Err(Error::BlockRange);
        }
        match &self.bad {
            Some(table) => Ok(table[block as usize / 8] & (1 << (block % 8)) != 0),
            None => self.bad_marker(block).map(|marker| marker != 0xff),
        }
    }
    pub fn is_free(&mut self, page: u32) -> Result<bool, Error<T::Error>> {
        let mut raw = [0; PAGE_BYTES];
        self.raw_read(page, 0, &mut raw)?;
        Ok(raw.iter().all(|&b| b == 0xff))
    }
    /// Dhara supplies ascending pages and never reprograms a physical page.
    /// One program carries data and a used-page marker in user OOB bytes 2..4.
    pub fn program_page(&mut self, page: u32, data: &[u8]) -> Result<(), Error<T::Error>> {
        if page >= PAGE_COUNT {
            return Err(Error::PageRange);
        }
        if data.len() != DATA_BYTES {
            return Err(Error::BufferSize);
        }
        if !self.writable {
            return Err(Error::WritesDisabled);
        }
        if self.is_bad(page / PAGES_PER_BLOCK)? {
            return Err(Error::BadBlock);
        }
        let mut load = [0xff; DATA_BYTES + 4];
        load[..DATA_BYTES].copy_from_slice(data);
        load[DATA_BYTES + 2..].fill(0);
        self.write(Write::Load(0), &load)?;
        self.write_enable()?;
        self.write(Write::Execute(page as u16), &[])?;
        let status = self.wait(PROGRAM_TIMEOUT_US, Error::ProgramTimeout)?;
        if status & 8 != 0 {
            return Err(Error::ProgramFailed);
        }
        Ok(())
    }
    pub fn erase_block(&mut self, block: u32) -> Result<(), Error<T::Error>> {
        if block >= BLOCKS {
            return Err(Error::BlockRange);
        }
        if !self.writable {
            return Err(Error::WritesDisabled);
        }
        if self.is_bad(block)? {
            return Err(Error::BadBlock);
        }
        self.write_enable()?;
        self.write(Write::Erase((block * PAGES_PER_BLOCK) as u16), &[])?;
        let status = self.wait(ERASE_TIMEOUT_US, Error::EraseTimeout)?;
        if status & 4 != 0 {
            return Err(Error::EraseFailed);
        }
        Ok(())
    }
    /// Retire a failed block without erasing its factory/data contents first.
    /// Raw OOB programming leaves data/ECC untouched; verify the marker.
    pub fn mark_bad(&mut self, block: u32) -> Result<(), Error<T::Error>> {
        if block >= BLOCKS {
            return Err(Error::BlockRange);
        }
        if !self.writable {
            return Err(Error::WritesDisabled);
        }
        if self.is_bad(block)? {
            return Ok(());
        }
        let id = self.normal()?;
        self.write_enable()?;
        let result = (|| {
            self.set_feature(0xb0, id.configuration & !0x10)?;
            self.write(Write::Load(DATA_BYTES as u16), &[0])?;
            self.write(Write::Enable, &[])?;
            if self.feature(0xc0)? & 3 != 2 {
                return Err(Error::WriteEnable);
            }
            self.write(Write::Execute((block * PAGES_PER_BLOCK) as u16), &[])?;
            let status = self.wait(PROGRAM_TIMEOUT_US, Error::ProgramTimeout)?;
            if status & 8 != 0 {
                return Err(Error::MarkerFailed);
            }
            Ok(())
        })();
        if self.set_feature(0xb0, id.configuration).is_err() {
            self.writable = false;
            return Err(Error::ConfigurationRestore);
        }
        result?;
        if self.bad_marker(block)? == 0xff {
            return Err(Error::MarkerFailed);
        }
        if let Some(table) = &mut self.bad {
            table[block as usize / 8] |= 1 << (block % 8);
        }
        Ok(())
    }

    fn feature(&mut self, address: u8) -> Result<u8, Error<T::Error>> {
        let mut value = [0];
        self.transport
            .read(Read::Feature(address), &mut value)
            .map_err(Error::Transport)?;
        Ok(value[0])
    }

    /// Output is valid only on Ok. On any error callers must discard the buffer.
    #[cfg(test)]
    pub fn read_page(
        &mut self,
        page: u32,
        output: &mut [u8],
    ) -> Result<PageStatus, Error<T::Error>> {
        if output.len() != PAGE_BYTES {
            return Err(Error::BufferSize);
        }
        self.read_range(page, 0, output)
    }

    /// Read only the requested ECC-protected main-area bytes. Dhara reads
    /// small metadata slices frequently; no full-page SPI transfer is needed.
    pub fn read_data(
        &mut self,
        page: u32,
        offset: usize,
        output: &mut [u8],
    ) -> Result<PageStatus, Error<T::Error>> {
        if offset
            .checked_add(output.len())
            .is_none_or(|end| end > DATA_BYTES)
            || output.is_empty()
        {
            return Err(Error::BufferSize);
        }
        self.read_range(page, offset as u16, output)
    }

    fn read_range(
        &mut self,
        page: u32,
        offset: u16,
        output: &mut [u8],
    ) -> Result<PageStatus, Error<T::Error>> {
        if page >= PAGE_COUNT {
            return Err(Error::PageRange);
        }
        self.normal()?;
        self.transport
            .load_page(page as u16)
            .map_err(Error::Transport)?;
        let status = self.wait(READ_TIMEOUT_US, Error::ReadTimeout)?;
        let extended = if (status >> 4) & 3 == 1 {
            Some(self.feature(0xf0)?)
        } else {
            None
        };
        let status = decode_ecc(status, extended)?;
        self.transport
            .read(Read::Cache(offset), output)
            .map_err(Error::Transport)?;
        Ok(status)
    }

    pub fn identify(&mut self) -> Result<Identity, Error<T::Error>> {
        self.verified_id = None;
        let mut id = [0; 4];
        self.transport
            .read(Read::JedecId, &mut id)
            .map_err(Error::Transport)?;
        // Open MISO commonly reads all ones or all zeroes. Unknown non-empty
        // IDs remain valid discovery results; never guess a supported part.
        if id == [0; 4] || id == [0xff; 4] {
            return Err(Error::NoResponse);
        }
        let mut repeat = [0; 4];
        self.transport
            .read(Read::JedecId, &mut repeat)
            .map_err(Error::Transport)?;
        if id != repeat {
            return Err(Error::UnstableId);
        }
        let status = self.feature(0xc0)?;
        if status & 1 != 0 {
            // Discovery does not issue a reset to interrupt an unknown operation.
            return Err(Error::Busy);
        }
        let identity = Identity {
            id,
            protection: self.feature(0xa0)?,
            configuration: self.feature(0xb0)?,
            status,
        };
        if identity.model().is_some() {
            self.verified_id = Some(id);
        }
        Ok(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn wire_phases_preserve_id_zero_address_and_cache_dummy_clocks() {
        assert_eq!(
            Read::JedecId.frame(),
            ReadFrame {
                opcode: 0x9f,
                address: 0,
                address_bits: 8,
                dummy_cycles: 0
            }
        );
        for address in [0xa0, 0xb0, 0xc0, 0xf0] {
            assert_eq!(
                Read::Feature(address).frame(),
                ReadFrame {
                    opcode: 0x0f,
                    address: address as u16,
                    address_bits: 8,
                    dummy_cycles: 0
                }
            );
        }
        assert_eq!(
            Read::Cache(2048).frame(),
            ReadFrame {
                opcode: 3,
                address: 2048,
                address_bits: 16,
                dummy_cycles: 8
            }
        );
        assert_eq!(
            Read::Cache(2048).frame_for(true),
            ReadFrame {
                opcode: 0x6b,
                address: 2048,
                address_bits: 16,
                dummy_cycles: 8,
            }
        );
        assert_eq!(Write::Load(2048).frame_for(true), (0x32, 2048, 16));
        assert_eq!(Write::Load(2048).frame(), (0x02, 2048, 16));
        assert_eq!(Read::JedecId.frame_for(true), Read::JedecId.frame());
        assert_eq!(
            Read::Feature(0xb0).frame_for(true),
            Read::Feature(0xb0).frame()
        );
    }

    struct Bus(VecDeque<(Read, Result<Vec<u8>, u8>)>);
    impl Transport for Bus {
        type Error = u8;
        fn write(&mut self, _: Write, _: &[u8]) -> Result<(), u8> {
            panic!("read-only discovery must not write")
        }
        fn read(&mut self, command: Read, bytes: &mut [u8]) -> Result<(), u8> {
            let (expected, result) = self.0.pop_front().expect("unexpected bus operation");
            assert_eq!(command, expected);
            bytes.copy_from_slice(&result?);
            Ok(())
        }
        fn load_page(&mut self, _: u16) -> Result<(), u8> {
            panic!("discovery must not load pages")
        }
        fn now_us(&self) -> u64 {
            0
        }
        fn delay_us(&mut self, _: u32) {
            panic!("discovery must not wait")
        }
    }
    fn bus(steps: Vec<(Read, Result<Vec<u8>, u8>)>) -> Nand<Bus> {
        Nand::new(Bus(steps.into()))
    }
    #[test]
    fn preserves_unknown_id_and_features_without_writes() {
        let raw = vec![0x52, 0xa7, 0x10, 0x93];
        let mut nand = bus(vec![
            (Read::JedecId, Ok(raw.clone())),
            (Read::JedecId, Ok(raw)),
            (Read::Feature(0xc0), Ok(vec![0xa0])),
            (Read::Feature(0xa0), Ok(vec![0x38])),
            (Read::Feature(0xb0), Ok(vec![0x10])),
        ]);
        assert_eq!(
            nand.identify(),
            Ok(Identity {
                id: [0x52, 0xa7, 0x10, 0x93],
                protection: 0x38,
                configuration: 0x10,
                status: 0xa0
            })
        );
        assert!(nand.transport.0.is_empty());
    }
    #[test]
    fn disconnected_and_unstable_devices_are_not_identified() {
        for raw in [[0; 4], [255; 4]] {
            assert_eq!(
                bus(vec![(Read::JedecId, Ok(raw.to_vec()))]).identify(),
                Err(Error::NoResponse)
            );
        }
        assert_eq!(
            bus(vec![
                (Read::JedecId, Ok(vec![1, 2, 3, 4])),
                (Read::JedecId, Ok(vec![1, 2, 3, 5]))
            ])
            .identify(),
            Err(Error::UnstableId)
        );
    }
    #[test]
    fn errors_and_busy_stop_discovery_without_resetting_the_chip() {
        assert_eq!(
            bus(vec![(Read::JedecId, Err(7))]).identify(),
            Err(Error::Transport(7))
        );
        assert_eq!(
            bus(vec![
                (Read::JedecId, Ok(vec![1, 2, 3, 4])),
                (Read::JedecId, Ok(vec![1, 2, 3, 4])),
                (Read::Feature(0xc0), Ok(vec![1]))
            ])
            .identify(),
            Err(Error::Busy)
        );
    }

    struct PageBus {
        id: [u8; 4],
        configuration: u8,
        status: u8,
        extended: u8,
        loaded: Option<u16>,
        now: u64,
        stuck_busy: bool,
        preempt_after_busy_us: u64,
        fail: Option<Read>,
        reads: Vec<Read>,
    }
    impl Default for PageBus {
        fn default() -> Self {
            Self {
                id: [0xc8, 0x91, 0xc8, 0x91],
                configuration: 0x10,
                status: 0,
                extended: 0,
                loaded: None,
                now: 0,
                stuck_busy: false,
                preempt_after_busy_us: 0,
                fail: None,
                reads: Vec::new(),
            }
        }
    }
    impl Transport for PageBus {
        type Error = u8;
        fn write(&mut self, _: Write, _: &[u8]) -> Result<(), u8> {
            panic!("normal page reads must not write")
        }
        fn read(&mut self, command: Read, bytes: &mut [u8]) -> Result<(), u8> {
            self.reads.push(command);
            if self.fail == Some(command) {
                return Err(9);
            }
            match command {
                Read::JedecId => bytes.copy_from_slice(&self.id),
                Read::Feature(0xa0) => bytes[0] = 0x38,
                Read::Feature(0xb0) => bytes[0] = self.configuration,
                Read::Feature(0xc0) => {
                    bytes[0] = if self.loaded.is_none() {
                        0
                    } else if self.stuck_busy || self.now < 120 {
                        1
                    } else {
                        self.status
                    };
                    // The device runs while a higher-priority task interrupts
                    // the caller after it has sampled the status register.
                    if bytes[0] & 1 != 0 {
                        self.now += std::mem::take(&mut self.preempt_after_busy_us);
                    }
                }
                Read::Feature(0xf0) => bytes[0] = self.extended,
                Read::Feature(_) => panic!("unexpected feature"),
                Read::Cache(offset) => {
                    assert!(self.loaded.is_some());
                    assert!(self.now >= 120);
                    for (i, byte) in bytes.iter_mut().enumerate() {
                        let i = usize::from(offset) + i;
                        *byte = (i ^ (i >> 8)) as u8;
                    }
                }
            }
            Ok(())
        }
        fn load_page(&mut self, page: u16) -> Result<(), u8> {
            assert!(self.loaded.is_none());
            self.loaded = Some(page);
            Ok(())
        }
        fn now_us(&self) -> u64 {
            self.now
        }
        fn delay_us(&mut self, us: u32) {
            self.now += u64::from(us);
        }
    }
    #[test]
    fn page_read_waits_for_ready_and_preserves_full_data_and_spare() {
        let mut nand = Nand::new(PageBus::default());
        let mut data = [0; PAGE_BYTES];
        let status = nand.read_page(PAGE_COUNT - 1, &mut data).unwrap();
        assert_eq!(nand.transport.loaded, Some(65535));
        assert_eq!(nand.transport.now, 120);
        assert_eq!((status.corrected_min, status.corrected_max), (0, 0));
        assert_eq!(status.extended_status, None);
        assert_eq!(nand.transport.reads.last(), Some(&Read::Cache(0)));
        for (i, byte) in data.iter().enumerate() {
            assert_eq!(*byte, (i ^ (i >> 8)) as u8);
        }
    }
    #[test]
    fn metadata_slices_preserve_column_and_main_area_bounds() {
        for (offset, length) in [(0, 4), (17, 33), (2000, 48)] {
            let mut nand = Nand::new(PageBus::default());
            let mut data = vec![0; length];
            nand.read_data(12, offset, &mut data).unwrap();
            assert_eq!(nand.transport.loaded, Some(12));
            assert_eq!(
                nand.transport.reads.last(),
                Some(&Read::Cache(offset as u16))
            );
            let expected: Vec<_> = (offset..offset + length)
                .map(|i| (i ^ (i >> 8)) as u8)
                .collect();
            assert_eq!(data, expected);
        }
        let mut nand = Nand::new(PageBus::default());
        for (offset, length) in [(DATA_BYTES - 1, 2), (usize::MAX, 1), (0, 0)] {
            assert_eq!(
                nand.read_data(0, offset, &mut vec![0; length]),
                Err(Error::BufferSize)
            );
        }
        assert_eq!(
            nand.read_data(PAGE_COUNT, 0, &mut [0; 4]),
            Err(Error::PageRange)
        );
        assert!(nand.transport.reads.is_empty());
    }
    #[test]
    fn shared_id_prefix_or_unsafe_configuration_never_loads_a_page() {
        for (bus, error) in [
            (
                PageBus {
                    id: [0xc8, 0x91, 0x01, 0xc8],
                    ..Default::default()
                },
                Error::UnknownModel,
            ),
            (
                PageBus {
                    id: [0x52, 0xa7, 0x10, 0x93],
                    ..Default::default()
                },
                Error::UnknownModel,
            ),
            (
                PageBus {
                    configuration: 0x50,
                    ..Default::default()
                },
                Error::OtpEnabled,
            ),
            (
                PageBus {
                    configuration: 0,
                    ..Default::default()
                },
                Error::EccDisabled,
            ),
        ] {
            let mut nand = Nand::new(bus);
            assert_eq!(nand.read_page(0, &mut [0; PAGE_BYTES]), Err(error));
            assert_eq!(nand.transport.loaded, None);
        }
    }
    #[test]
    fn page_and_buffer_bounds_reject_before_bus_access() {
        let mut nand = Nand::new(PageBus::default());
        assert_eq!(
            nand.read_page(PAGE_COUNT, &mut [0; PAGE_BYTES]),
            Err(Error::PageRange)
        );
        assert_eq!(
            nand.read_page(u32::MAX, &mut [0; PAGE_BYTES]),
            Err(Error::PageRange)
        );
        assert_eq!(
            nand.read_page(0, &mut [0; DATA_BYTES]),
            Err(Error::BufferSize)
        );
        assert!(nand.transport.reads.is_empty());
    }
    #[test]
    fn ecc_reports_documented_ranges_and_does_not_read_uncorrectable_data() {
        for (status, extended, expected) in [
            (0x00, 0, (0, 0)),
            (0x10, 0, (1, 4)),
            (0x10, 0x10, (5, 5)),
            (0x10, 0x20, (6, 6)),
            (0x10, 0x30, (7, 7)),
            (0x30, 0, (8, 8)),
        ] {
            let mut nand = Nand::new(PageBus {
                status,
                extended,
                ..Default::default()
            });
            let got = nand.read_page(0, &mut [0; PAGE_BYTES]).unwrap();
            assert_eq!((got.corrected_min, got.corrected_max), expected);
            assert_eq!(
                got.extended_status,
                if status == 0x10 { Some(extended) } else { None }
            );
        }
        let mut nand = Nand::new(PageBus {
            status: 0x20,
            ..Default::default()
        });
        assert_eq!(
            nand.read_page(0, &mut [0; PAGE_BYTES]),
            Err(Error::Uncorrectable)
        );
        assert!(!nand.transport.reads.contains(&Read::Cache(0)));
    }
    #[test]
    fn timeout_and_transport_failure_never_return_valid_page_data() {
        let mut nand = Nand::new(PageBus {
            stuck_busy: true,
            ..Default::default()
        });
        assert_eq!(
            nand.read_page(0, &mut [0; PAGE_BYTES]),
            Err(Error::ReadTimeout)
        );
        assert_eq!(nand.transport.now, READ_TIMEOUT_US);
        assert!(!nand.transport.reads.contains(&Read::Cache(0)));
        for fail in [Read::Cache(0), Read::Feature(0xf0)] {
            let mut nand = Nand::new(PageBus {
                status: 0x10,
                fail: Some(fail),
                ..Default::default()
            });
            assert_eq!(
                nand.read_page(0, &mut [0; PAGE_BYTES]),
                Err(Error::Transport(9))
            );
        }
    }

    #[test]
    fn page_read_rechecks_ready_after_preemption_crosses_deadline() {
        let mut nand = Nand::new(PageBus {
            preempt_after_busy_us: READ_TIMEOUT_US * 2,
            ..Default::default()
        });
        let mut data = [0; 16];
        nand.read_data(12, 17, &mut data).unwrap();
        for (i, byte) in data.iter().enumerate() {
            assert_eq!(*byte, (i + 17) as u8);
        }
        assert!(nand.transport.now > READ_TIMEOUT_US);
        assert_eq!(nand.transport.reads.last(), Some(&Read::Cache(17)));
    }

    #[test]
    fn wait_uses_status_sampled_after_deadline_to_decide_timeout() {
        for (timeout, error) in [
            (READ_TIMEOUT_US, Error::ReadTimeout),
            (PROGRAM_TIMEOUT_US, Error::ProgramTimeout),
            (ERASE_TIMEOUT_US, Error::EraseTimeout),
        ] {
            for stuck_busy in [false, true] {
                let mut nand = Nand::new(PageBus {
                    loaded: Some(0),
                    status: 0x10,
                    stuck_busy,
                    preempt_after_busy_us: timeout * 2,
                    ..Default::default()
                });
                assert_eq!(
                    nand.wait(timeout, error),
                    if stuck_busy { Err(error) } else { Ok(0x10) }
                );
                assert_eq!(
                    nand.transport.reads,
                    [Read::Feature(0xc0), Read::Feature(0xc0)]
                );
            }
        }
    }
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Chip {
        configuration: u8,
        protection: u8,
        status: u8,
        pages: BTreeMap<u16, Vec<u8>>,
        cache: [u8; PAGE_BYTES],
        writes: Vec<Write>,
        loads: Vec<u16>,
        erases: Vec<u16>,
        fail_program: bool,
        fail_erase: bool,
        fail_restore: bool,
        fail_raw_read: bool,
        stuck: bool,
        now: u64,
        id_reads: u32,
    }
    impl Default for Chip {
        fn default() -> Self {
            Self {
                configuration: 0x10,
                protection: 0x38,
                status: 0,
                pages: BTreeMap::new(),
                cache: [255; PAGE_BYTES],
                writes: Vec::new(),
                loads: Vec::new(),
                erases: Vec::new(),
                fail_program: false,
                fail_erase: false,
                fail_restore: false,
                fail_raw_read: false,
                stuck: false,
                now: 0,
                id_reads: 0,
            }
        }
    }
    impl Transport for Chip {
        type Error = u8;
        fn read(&mut self, command: Read, bytes: &mut [u8]) -> Result<(), u8> {
            match command {
                Read::JedecId => {
                    self.id_reads += 1;
                    bytes.copy_from_slice(&[0xc8, 0x91, 0xc8, 0x91]);
                }
                Read::Feature(0xa0) => bytes[0] = self.protection,
                Read::Feature(0xb0) => bytes[0] = self.configuration,
                Read::Feature(0xc0) => bytes[0] = self.status,
                Read::Feature(0xf0) => bytes[0] = 0,
                Read::Cache(offset) => {
                    if self.fail_raw_read && self.configuration & 0x10 == 0 {
                        return Err(7);
                    }
                    bytes.copy_from_slice(
                        &self.cache[offset as usize..offset as usize + bytes.len()],
                    );
                }
                _ => panic!("invalid read"),
            }
            Ok(())
        }
        fn write(&mut self, command: Write, bytes: &[u8]) -> Result<(), u8> {
            self.writes.push(command);
            match command {
                Write::Feature(0xa0) => self.protection = bytes[0],
                Write::Feature(0xb0) => {
                    if self.fail_restore && bytes[0] & 0x10 != 0 {
                        return Err(8);
                    }
                    self.configuration = bytes[0];
                }
                Write::Enable => self.status = (self.status & !3) | 2,
                Write::Load(offset) => {
                    self.cache.fill(255);
                    self.cache[offset as usize..offset as usize + bytes.len()]
                        .copy_from_slice(bytes);
                }
                Write::Execute(page) => {
                    assert_eq!(self.status & 3, 2);
                    assert_eq!(self.protection & 0x7e, 0);
                    self.status = if self.stuck {
                        1
                    } else if self.fail_program {
                        8
                    } else {
                        0
                    };
                    if !self.fail_program && !self.stuck {
                        let previous = self
                            .pages
                            .entry(page)
                            .or_insert_with(|| vec![255; PAGE_BYTES]);
                        for (dst, src) in previous.iter_mut().zip(self.cache) {
                            *dst &= src;
                        }
                    }
                }
                Write::Erase(page) => {
                    assert_eq!(self.status & 3, 2);
                    assert_eq!(page % 64, 0);
                    self.status = if self.stuck {
                        1
                    } else if self.fail_erase {
                        4
                    } else {
                        0
                    };
                    self.erases.push(page);
                    if !self.fail_erase && !self.stuck {
                        for p in page..=page + 63 {
                            self.pages.remove(&p);
                        }
                    }
                }
                _ => panic!("invalid write"),
            }
            Ok(())
        }
        fn load_page(&mut self, page: u16) -> Result<(), u8> {
            self.loads.push(page);
            self.cache.fill(255);
            if let Some(data) = self.pages.get(&page) {
                self.cache.copy_from_slice(data);
            }
            self.status = 0;
            Ok(())
        }
        fn now_us(&self) -> u64 {
            self.now
        }
        fn delay_us(&mut self, us: u32) {
            self.now += u64::from(us);
        }
    }
    fn ready() -> Nand<Chip> {
        let mut nand = Nand::new(Chip::default());
        nand.prepare_writes().unwrap();
        nand.transport.writes.clear();
        nand
    }
    #[test]
    fn scans_all_markers_before_unlock_and_never_erases_bad_blocks() {
        let mut chip = Chip::default();
        let mut first = vec![255; PAGE_BYTES];
        first[DATA_BYTES] = 0;
        chip.pages.insert(128, first);
        let mut nand = Nand::new(chip);
        assert_eq!(nand.erase_block(0), Err(Error::WritesDisabled));
        nand.prepare_writes().unwrap();
        assert_eq!(nand.bad_count(), Some(1));
        assert_eq!(
            nand.transport.loads,
            (0..BLOCKS).map(|b| (b * 64) as u16).collect::<Vec<_>>()
        );
        assert_eq!(nand.transport.writes.last(), Some(&Write::Feature(0xa0)));
        assert_eq!(nand.erase_block(2), Err(Error::BadBlock));
        assert_eq!(
            nand.program_page(128, &[0; DATA_BYTES]),
            Err(Error::BadBlock)
        );
        assert!(nand.transport.erases.is_empty());
        assert_eq!(nand.transport.configuration, 0x10);
    }
    #[test]
    fn program_load_enable_execute_preserves_bad_marker_and_sets_used_marker() {
        let mut nand = ready();
        nand.program_page(66, &[42; DATA_BYTES]).unwrap();
        assert_eq!(
            nand.transport.writes,
            vec![Write::Load(0), Write::Enable, Write::Execute(66)]
        );
        let page = nand.transport.pages.get(&66).unwrap();
        assert_eq!(&page[..DATA_BYTES], &[42; DATA_BYTES]);
        assert_eq!(&page[DATA_BYTES..DATA_BYTES + 4], &[255, 255, 0, 0]);
        assert!(!nand.is_free(66).unwrap());
        assert!(nand.is_free(67).unwrap());
        nand.erase_block(1).unwrap();
        assert_eq!(nand.transport.erases, [64]);
        assert!(nand.is_free(66).unwrap());
    }
    #[test]
    fn all_ff_payload_is_still_a_used_physical_page() {
        let mut nand = ready();
        nand.program_page(0, &[255; DATA_BYTES]).unwrap();
        assert!(!nand.is_free(0).unwrap());
    }
    #[test]
    fn completed_media_failures_and_timeouts_are_distinct() {
        let mut nand = ready();
        nand.transport.fail_program = true;
        assert_eq!(
            nand.program_page(0, &[7; DATA_BYTES]),
            Err(Error::ProgramFailed)
        );
        nand.transport.fail_erase = true;
        assert_eq!(nand.erase_block(0), Err(Error::EraseFailed));
        let mut nand = ready();
        nand.transport.stuck = true;
        assert_eq!(
            nand.program_page(0, &[7; DATA_BYTES]),
            Err(Error::ProgramTimeout)
        );
        let mut nand = ready();
        nand.transport.stuck = true;
        assert_eq!(nand.erase_block(0), Err(Error::EraseTimeout));
    }
    #[test]
    fn marker_retirement_is_persistent_without_erasing_existing_data() {
        let mut nand = ready();
        nand.program_page(0, &[31; DATA_BYTES]).unwrap();
        nand.mark_bad(0).unwrap();
        assert_eq!(nand.bad_count(), Some(1));
        assert!(nand.is_bad(0).unwrap());
        assert_eq!(nand.transport.configuration, 0x10);
        let page = nand.transport.pages.get(&0).unwrap();
        assert_eq!(&page[..DATA_BYTES], &[31; DATA_BYTES]);
        assert_eq!(page[DATA_BYTES], 0);
        let mut restarted = Nand::new(nand.transport);
        restarted.prepare_writes().unwrap();
        assert_eq!(restarted.erase_block(0), Err(Error::BadBlock));
        assert!(restarted.transport.erases.is_empty());
    }
    #[test]
    fn raw_read_errors_restore_ecc_and_restore_failure_revokes_writes() {
        let mut nand = ready();
        nand.transport.fail_raw_read = true;
        assert_eq!(nand.is_free(0), Err(Error::Transport(7)));
        assert_eq!(nand.transport.configuration, 0x10);
        nand.transport.fail_raw_read = false;
        nand.transport.fail_restore = true;
        assert_eq!(nand.is_free(0), Err(Error::ConfigurationRestore));
        assert!(!nand.writable);
    }
    #[test]
    fn invalid_ranges_do_not_access_the_bus() {
        let mut nand = ready();
        assert_eq!(nand.erase_block(BLOCKS), Err(Error::BlockRange));
        assert_eq!(
            nand.program_page(PAGE_COUNT, &[0; DATA_BYTES]),
            Err(Error::PageRange)
        );
        assert_eq!(nand.program_page(0, &[0; 1]), Err(Error::BufferSize));
        assert!(nand.transport.writes.is_empty());
    }

    #[test]
    fn quad_bootstrap_preserves_modes_and_checks_live_qe_after_transport_transfer() {
        let mut nand = Nand::new(Chip {
            configuration: 0x12,
            ..Chip::default()
        });
        nand.enable_quad().unwrap();
        assert_eq!(nand.transport.configuration, 0x13);
        assert_eq!(nand.transport.writes, [Write::Feature(0xb0)]);
        assert!(!nand.writable);
        let mut nand = nand.map_transport(|chip| chip);
        nand.read_data(0, 0, &mut [0; 16]).unwrap();
        assert_eq!(nand.transport.id_reads, 2);
        nand.transport.configuration &= !1;
        assert_eq!(
            nand.read_data(0, 0, &mut [0; 16]),
            Err(Error::FeatureVerify)
        );
    }

    #[test]
    fn cached_identity_never_caches_mutable_modes_or_protection() {
        let mut nand = ready();
        assert_eq!(nand.transport.id_reads, 2);
        for (configuration, error) in [(0, Error::EccDisabled), (0x50, Error::OtpEnabled)] {
            nand.transport.configuration = configuration;
            assert_eq!(nand.read_data(0, 0, &mut [0; 16]), Err(error));
        }
        nand.transport.configuration = 0x10;
        nand.transport.protection = 0x38;
        assert_eq!(nand.erase_block(0), Err(Error::Protected));
        nand.transport.protection = 0;
        nand.transport.status = 1;
        assert_eq!(nand.read_data(0, 0, &mut [0; 16]), Err(Error::Busy));
        assert_eq!(nand.transport.id_reads, 2);
    }

    #[test]
    fn failed_quad_configuration_is_not_enabled() {
        let mut nand = Nand::new(Chip {
            fail_restore: true,
            ..Chip::default()
        });
        assert_eq!(nand.enable_quad(), Err(Error::Transport(8)));
        assert!(!nand.quad);
        assert!(!nand.writable);
    }
}
