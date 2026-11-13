//! Writable mode is explicitly entered and restricted to main + boot journal.
use super::storage::Nor;
use crate::{
    install::{Installer, Manifest},
    protocol::{Packet, Status},
};
use alloc::vec::Vec;
use boot_contract::{Flash, JOURNAL, MAIN_BASE, MAIN_BYTES, SECTOR};
use core::sync::atomic::Ordering;
use fs::Errno;

pub const ENTER: u8 = 0x10;
pub const DATA: u8 = 0x11;
pub const FINISH: u8 = 0x12;
pub const LEAVE: u8 = 0x13;
pub const BOOT: u8 = 0x14;
pub const ROM_DOWNLOAD: u8 = 0x15;
pub struct InstallMode {
    session: Option<Installer>,
    flash: MainFlash,
}
impl Default for InstallMode {
    fn default() -> Self {
        Self {
            session: None,
            flash: MainFlash { unlocked: false },
        }
    }
}
impl InstallMode {
    pub fn disconnect(&mut self) {
        self.session = None;
        super::WRITE_MODE.store(false, Ordering::Release);
    }
    pub async fn handle(&mut self, p: &Packet) -> Option<Packet> {
        if p.operation < ENTER {
            return None;
        }
        let result = self.execute(p).await;
        let (status, data) = match result {
            Ok(data) => (Status::Ok, data),
            Err(s) => (s, Vec::new()),
        };
        Some(Packet {
            id: p.id,
            operation: p.operation,
            status: status as u8,
            data,
        })
    }
    async fn execute(&mut self, p: &Packet) -> Result<Vec<u8>, Status> {
        if p.status != 0 {
            return Err(Status::Invalid);
        }
        match p.operation {
            ENTER if p.data.len() == 52 && &p.data[..8] == b"DGINST3\0" => {
                let mut table = [0; 0xc00];
                Nor::read(0x8000, &mut table).map_err(|_| Status::Io)?;
                if !boot_contract::single_main_layout(&table) {
                    return Err(Status::Invalid);
                }
                let m = Manifest {
                    layout: p.data[8..16].try_into().unwrap(),
                    bytes: u32::from_le_bytes(p.data[16..20].try_into().unwrap()),
                    sha256: p.data[20..52].try_into().unwrap(),
                };
                self.disconnect();
                self.session = Some(Installer::begin(&mut self.flash, m).map_err(|_| Status::Io)?);
                super::WRITE_MODE.store(true, Ordering::Release);
                super::INSTALL_PHASE.store(2, Ordering::Release);
                super::INSTALL_POSITION.store(0, Ordering::Release);
                super::INSTALL_TOTAL.store(m.bytes, Ordering::Release);
                Ok(b"installing".into())
            }
            DATA if p.data.len() > 4 => {
                let offset = u32::from_le_bytes(p.data[..4].try_into().unwrap());
                let session = self.session.as_mut().ok_or(Status::ReadOnly)?;
                session
                    .push(&mut self.flash, offset, &p.data[4..])
                    .map_err(|_| Status::Io)?;
                super::INSTALL_POSITION.store(session.received(), Ordering::Release);
                Ok(session.received().to_le_bytes().into())
            }
            FINISH if p.data.is_empty() => {
                let session = self.session.take().ok_or(Status::ReadOnly)?;
                let result = async {
                    let mut verification = session
                        .start_verify::<Errno>()
                        .map_err(|_| Status::Invalid)?;
                    super::INSTALL_PHASE.store(3, Ordering::Release);
                    loop {
                        if super::CANCELLED.load(Ordering::Acquire)
                            || !super::CONNECTED.load(Ordering::Acquire)
                        {
                            return Err(Status::Cancelled);
                        }
                        let result = verification.step(&mut self.flash).map_err(|_| Status::Io)?;
                        super::INSTALL_POSITION.store(verification.checked(), Ordering::Release);
                        if result.is_some() {
                            return Ok(());
                        }
                        embassy_time::Timer::after_millis(1).await;
                    }
                }
                .await;
                super::WRITE_MODE.store(false, Ordering::Release);
                super::INSTALL_PHASE.store(if result.is_ok() { 4 } else { 5 }, Ordering::Release);
                result?;
                Ok(b"trial-ready".into())
            }
            LEAVE if p.data.is_empty() => {
                self.disconnect();
                Ok(b"read-only".into())
            }
            BOOT if p.data == b"BOOT" && self.session.is_none() => {
                let (_, record) = boot_contract::read(&mut self.flash)
                    .map_err(|_| Status::Io)?
                    .ok_or(Status::Unavailable)?;
                if !matches!(
                    record.state,
                    boot_contract::State::Trial | boot_contract::State::Ready
                ) {
                    return Err(Status::Invalid);
                }
                Ok(b"reboot".into())
            }
            ROM_DOWNLOAD if p.data == b"ROM!" && self.session.is_none() => {
                if !board_s31::reset::download_available() {
                    return Err(Status::Unavailable);
                }
                Ok(b"rom-download".into())
            }
            _ => Err(Status::Invalid),
        }
    }
}
pub(super) struct MainFlash {
    unlocked: bool,
}
impl MainFlash {
    pub(super) fn new() -> Self {
        Self { unlocked: false }
    }
    fn allowed(address: u32, len: usize) -> bool {
        let Some(end) = address.checked_add(len as u32) else {
            return false;
        };
        (address >= MAIN_BASE && end <= MAIN_BASE + MAIN_BYTES)
            || JOURNAL
                .iter()
                .any(|&base| address >= base && end <= base + boot_contract::RECORD as u32)
    }
    fn unlock(&mut self) -> Result<(), Errno> {
        if !self.unlocked {
            critical_section::with(|_| unsafe { esp_storage::ll::spiflash_unlock() })
                .map_err(|_| Errno::IO)?;
            self.unlocked = true;
        }
        Ok(())
    }
}
impl Flash for MainFlash {
    type Error = Errno;
    fn read(&mut self, a: u32, out: &mut [u8]) -> Result<(), Errno> {
        if !Self::allowed(a, out.len()) {
            return Err(Errno::INVAL);
        }
        Nor::read(a, out)
    }
    fn erase(&mut self, a: u32) -> Result<(), Errno> {
        if a % SECTOR != 0
            || !(JOURNAL.contains(&a) || (a >= MAIN_BASE && a + SECTOR <= MAIN_BASE + MAIN_BYTES))
        {
            return Err(Errno::INVAL);
        }
        self.unlock()?;
        critical_section::with(|_| unsafe { esp_storage::ll::spiflash_erase_sector(a / SECTOR) })
            .map_err(|_| Errno::IO)
    }
    fn program(&mut self, a: u32, bytes: &[u8]) -> Result<(), Errno> {
        if a % 4 != 0
            || bytes.len() % 4 != 0
            || bytes.len() > 4096
            || !Self::allowed(a, bytes.len())
        {
            return Err(Errno::INVAL);
        }
        let mut words = [0u32; 1024];
        for (i, c) in bytes.chunks_exact(4).enumerate() {
            words[i] = u32::from_le_bytes(c.try_into().unwrap());
        }
        self.unlock()?;
        critical_section::with(|_| unsafe {
            esp_storage::ll::spiflash_write(a, words.as_ptr().cast(), bytes.len() as u32)
        })
        .map_err(|_| Errno::IO)
    }
}
