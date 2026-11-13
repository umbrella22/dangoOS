//! S31 single-main boot journal. Two erase sectors, body verification followed
//! by a separate commit word. All offsets describe on-flash bytes, not repr(C).
#![no_std]
pub mod display;
pub mod image;
pub mod partition;
pub const LAYOUT: &[u8; 8] = b"S31ONE03";
pub const MAGIC: &[u8; 8] = b"DGBOOT3\0";
pub const COMMIT: &[u8; 4] = b"DONE";
pub const BODY: usize = 96;
pub const RECORD: usize = 100;
pub const SECTOR: u32 = 4096;
pub const JOURNAL: [u32; 2] = [0x9000, 0xa000];
pub const RECOVERY_BASE: u32 = 0x20000;
pub const RECOVERY_BYTES: u32 = 0x1c0000;
pub const MAIN_BASE: u32 = 0x210000;
pub const MAIN_BYTES: u32 = 0xc00000;
pub const SYSTEM_BASE: u32 = 0xe10000;
pub const SYSTEM_BYTES: u32 = 0x1f0000;
/// Path inside the NAND data volume. Staged records refer to this closed file.
pub const STAGED_IMAGE: &str = "/.update/firmware.bin";

/// The installer may run in a development slot of an older image, but it may
/// write main only when the on-flash partition table has the new contract.
pub fn single_main_layout(table: &[u8]) -> bool {
    partition::verify(table)
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Rescue = 0,
    Installing = 1,
    Trial = 2,
    Testing = 3,
    Ready = 4,
    Staged = 5,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub generation: u64,
    pub state: State,
    pub image_bytes: u32,
    pub sha256: [u8; 32],
}
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
impl Record {
    pub const fn rescue() -> Self {
        Self {
            generation: 0,
            state: State::Rescue,
            image_bytes: 0,
            sha256: [0; 32],
        }
    }
    pub fn body(self) -> [u8; BODY] {
        let mut b = [0; BODY];
        b[..8].copy_from_slice(MAGIC);
        b[8..16].copy_from_slice(LAYOUT);
        b[16..24].copy_from_slice(&self.generation.to_le_bytes());
        b[24..28].copy_from_slice(&(self.state as u32).to_le_bytes());
        b[28..32].copy_from_slice(&self.image_bytes.to_le_bytes());
        b[32..64].copy_from_slice(&self.sha256);
        let crc = crc32(&b[..92]);
        b[92..96].copy_from_slice(&crc.to_le_bytes());
        b
    }
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != RECORD
            || &b[..8] != MAGIC
            || &b[8..16] != LAYOUT
            || &b[96..] != COMMIT
            || b[64..92].iter().any(|&v| v != 0)
            || crc32(&b[..92]) != u32::from_le_bytes(b[92..96].try_into().ok()?)
        {
            return None;
        }
        let state = match u32::from_le_bytes(b[24..28].try_into().ok()?) {
            0 => State::Rescue,
            1 => State::Installing,
            2 => State::Trial,
            3 => State::Testing,
            4 => State::Ready,
            5 => State::Staged,
            _ => return None,
        };
        let record = Self {
            generation: u64::from_le_bytes(b[16..24].try_into().ok()?),
            state,
            image_bytes: u32::from_le_bytes(b[28..32].try_into().ok()?),
            sha256: b[32..64].try_into().ok()?,
        };
        if record.state != State::Rescue
            && (record.image_bytes == 0 || record.image_bytes > MAIN_BYTES)
        {
            return None;
        }
        Some(record)
    }
    pub fn next(self, state: State) -> Option<Self> {
        Some(Self {
            generation: self.generation.checked_add(1)?,
            state,
            ..self
        })
    }
}
pub fn latest(a: Option<Record>, b: Option<Record>) -> Option<(usize, Record)> {
    match (a, b) {
        (Some(a), Some(b)) if a.generation == b.generation && a != b => None,
        (Some(a), Some(b)) if b.generation > a.generation => Some((1, b)),
        (Some(a), _) => Some((0, a)),
        (_, Some(b)) => Some((1, b)),
        _ => None,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Recovery,
    Main,
    ConsumeTrial,
}
pub fn target(record: Option<Record>, manual: bool, image_valid: bool) -> Target {
    if manual || !image_valid {
        return Target::Recovery;
    }
    match record.map(|r| r.state) {
        Some(State::Ready) => Target::Main,
        Some(State::Trial) => Target::ConsumeTrial,
        _ => Target::Recovery,
    }
}
pub trait Flash {
    type Error;
    fn read(&mut self, address: u32, out: &mut [u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, address: u32) -> Result<(), Self::Error>;
    fn program(&mut self, address: u32, data: &[u8]) -> Result<(), Self::Error>;
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Io(E),
    Verify,
    Generation,
}
pub fn read<F: Flash>(f: &mut F) -> Result<Option<(usize, Record)>, Error<F::Error>> {
    let mut a = [0; RECORD];
    let mut b = [0; RECORD];
    f.read(JOURNAL[0], &mut a).map_err(Error::Io)?;
    f.read(JOURNAL[1], &mut b).map_err(Error::Io)?;
    let (a, b) = (Record::decode(&a), Record::decode(&b));
    if matches!((a,b),(Some(a),Some(b)) if a.generation==b.generation && a!=b) {
        return Err(Error::Verify);
    }
    Ok(latest(a, b))
}
pub fn append<F: Flash>(
    f: &mut F,
    state: State,
    image_bytes: u32,
    sha256: [u8; 32],
) -> Result<Record, Error<F::Error>> {
    let previous = read(f)?;
    let (index, generation) = match previous {
        Some((i, r)) => (1 - i, r.generation.checked_add(1).ok_or(Error::Generation)?),
        None => (0, 0),
    };
    let record = Record {
        generation,
        state,
        image_bytes,
        sha256,
    };
    let body = record.body();
    // Reject invalid records before erasing either sector.
    let mut complete = [0; RECORD];
    complete[..BODY].copy_from_slice(&body);
    complete[BODY..].copy_from_slice(COMMIT);
    if Record::decode(&complete) != Some(record) {
        return Err(Error::Verify);
    }
    let address = JOURNAL[index];
    f.erase(address).map_err(Error::Io)?;
    f.program(address, &body).map_err(Error::Io)?;
    let mut verify = [0; RECORD];
    f.read(address, &mut verify).map_err(Error::Io)?;
    if verify[..BODY] != body || verify[BODY..] != [255; 4] {
        return Err(Error::Verify);
    }
    f.program(address + BODY as u32, COMMIT)
        .map_err(Error::Io)?;
    f.read(address, &mut verify).map_err(Error::Io)?;
    if Record::decode(&verify) != Some(record) {
        return Err(Error::Verify);
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use self::std::vec;
    use self::std::vec::Vec;
    use super::*;
    #[derive(Clone)]
    struct Nor {
        bytes: Vec<u8>,
        budget: Option<usize>,
    }
    impl Nor {
        fn step(&mut self) -> Result<(), ()> {
            if let Some(left) = &mut self.budget {
                if *left == 0 {
                    return Err(());
                }
                *left -= 1;
            }
            Ok(())
        }
    }
    impl Flash for Nor {
        type Error = ();
        fn read(&mut self, a: u32, b: &mut [u8]) -> Result<(), ()> {
            b.copy_from_slice(&self.bytes[a as usize..a as usize + b.len()]);
            Ok(())
        }
        fn erase(&mut self, a: u32) -> Result<(), ()> {
            for i in 0..4096 {
                self.step()?;
                self.bytes[a as usize + i] = 255;
            }
            Ok(())
        }
        fn program(&mut self, a: u32, b: &[u8]) -> Result<(), ()> {
            for (i, &v) in b.iter().enumerate() {
                self.step()?;
                self.bytes[a as usize + i] &= v;
            }
            Ok(())
        }
    }
    #[test]
    fn every_torn_journal_update_retains_a_committed_predecessor() {
        let mut disk = Nor {
            bytes: vec![255; 0xb000],
            budget: None,
        };
        let old = append(&mut disk, State::Ready, 8192, [29; 32]).unwrap();
        for cut in 0..=4096 + RECORD {
            let mut torn = disk.clone();
            torn.budget = Some(cut);
            let _ = append(&mut torn, State::Installing, 16384, [42; 32]);
            let (_, current) = read(&mut torn).unwrap().unwrap();
            if cut < 4096 + RECORD {
                assert_eq!(current, old)
            } else {
                assert_eq!(current.state, State::Installing)
            }
        }
    }
    #[test]
    fn trial_is_consumed_once_and_failed_trial_cannot_boot_main_again() {
        let r = Record {
            state: State::Trial,
            image_bytes: 4096,
            ..Record::rescue()
        };
        assert_eq!(target(Some(r), false, true), Target::ConsumeTrial);
        assert_eq!(
            target(r.next(State::Testing), false, true),
            Target::Recovery
        );
        assert_eq!(target(r.next(State::Ready), false, true), Target::Main);
        assert_eq!(target(r.next(State::Ready), true, true), Target::Recovery);
        assert_eq!(target(r.next(State::Ready), false, false), Target::Recovery);
        assert_eq!(target(None, false, true), Target::Recovery);
    }
    #[test]
    fn layout_crc_commit_and_capacity_are_part_of_record_identity() {
        let record = Record {
            generation: 100,
            state: State::Trial,
            image_bytes: MAIN_BYTES,
            sha256: [59; 32],
        };
        let mut bytes = [0; RECORD];
        bytes[..BODY].copy_from_slice(&record.body());
        bytes[BODY..].copy_from_slice(COMMIT);
        assert_eq!(Record::decode(&bytes), Some(record));
        for i in 0..RECORD {
            let mut bad = bytes;
            bad[i] ^= 1;
            assert!(Record::decode(&bad).is_none(), "offset {i}");
        }
    }
}
