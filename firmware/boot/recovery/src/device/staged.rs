//! Consume one closed NAND package. Preflight never erases main; each install
//! or readback step returns to the media loop for status and cancellation.
use super::{
    install::MainFlash,
    storage::{Nor, Storage},
};
use crate::install::{Installer, Manifest, Preflight, Verification};
use boot_contract::{Record, State};
use core::sync::atomic::Ordering;
use fs::Errno;

enum Phase {
    Preflight(Preflight),
    Write(Installer),
    Verify(Verification),
}
#[derive(Default)]
pub struct Staged {
    started: bool,
    phase: Option<Phase>,
}
impl Staged {
    pub fn active(&self) -> bool {
        self.phase.is_some()
    }
    pub fn started(&self) -> bool {
        self.started
    }
    pub fn start(&mut self, storage: &mut Storage) {
        self.started = true;
        let mut table = [0; 0xc00];
        if Nor::read(0x8000, &mut table).is_err() || !boot_contract::single_main_layout(&table) {
            return;
        }
        let mut flash = MainFlash::new();
        let Ok(Some((_, record))) = boot_contract::read(&mut flash) else {
            return;
        };
        if record.state != State::Staged {
            return;
        }
        // Consume the request before mounting. A bad package or explicit
        // cancellation must not retry itself on every subsequent reboot.
        if boot_contract::append(&mut flash, State::Rescue, 0, [0; 32]).is_err() {
            super::INSTALL_PHASE.store(5, Ordering::Release);
            return;
        }
        let manifest = Manifest {
            layout: *boot_contract::LAYOUT,
            bytes: record.image_bytes,
            sha256: record.sha256,
        };
        let Ok(preflight) = Preflight::new::<Errno>(manifest) else {
            self.cancel(storage, 5);
            return;
        };
        if storage.open_staged(manifest.bytes).is_err() {
            self.cancel(storage, 5);
            return;
        }
        super::WRITE_MODE.store(true, Ordering::Release);
        super::INSTALL_PHASE.store(1, Ordering::Release);
        super::INSTALL_POSITION.store(0, Ordering::Release);
        super::INSTALL_TOTAL.store(manifest.bytes, Ordering::Release);
        self.phase = Some(Phase::Preflight(preflight));
    }
    pub fn cancel(&mut self, storage: &mut Storage, status: u8) {
        self.phase = None;
        let closed = storage.close_staged().is_ok();
        super::WRITE_MODE.store(false, Ordering::Release);
        super::INSTALL_PHASE.store(if closed { status } else { 5 }, Ordering::Release);
    }
    fn advance(&mut self, storage: &mut Storage) -> Result<Option<Record>, ()> {
        let Some(phase) = self.phase.take() else {
            return Ok(None);
        };
        let mut flash = MainFlash::new();
        self.phase = Some(match phase {
            Phase::Preflight(mut check) => {
                let mut bytes = [0; 4096];
                let n = storage.read_staged(&mut bytes).map_err(|_| ())?;
                check.feed::<Errno>(&bytes[..n]).map_err(|_| ())?;
                super::INSTALL_POSITION.store(check.received(), Ordering::Release);
                if check.received() == super::INSTALL_TOTAL.load(Ordering::Acquire) {
                    let manifest = check.finish::<Errno>().map_err(|_| ())?;
                    storage.rewind_staged().map_err(|_| ())?;
                    let installer = Installer::begin(&mut flash, manifest).map_err(|_| ())?;
                    super::INSTALL_PHASE.store(2, Ordering::Release);
                    super::INSTALL_POSITION.store(0, Ordering::Release);
                    Phase::Write(installer)
                } else {
                    Phase::Preflight(check)
                }
            }
            Phase::Write(mut installer) => {
                let mut bytes = [0; 4096];
                let n = storage.read_staged(&mut bytes).map_err(|_| ())?;
                installer
                    .push(&mut flash, installer.received(), &bytes[..n])
                    .map_err(|_| ())?;
                super::INSTALL_POSITION.store(installer.received(), Ordering::Release);
                if installer.received() == super::INSTALL_TOTAL.load(Ordering::Acquire) {
                    storage.close_staged().map_err(|_| ())?;
                    super::INSTALL_PHASE.store(3, Ordering::Release);
                    super::INSTALL_POSITION.store(0, Ordering::Release);
                    Phase::Verify(installer.start_verify::<Errno>().map_err(|_| ())?)
                } else {
                    Phase::Write(installer)
                }
            }
            Phase::Verify(mut verification) => {
                let result = verification.step(&mut flash).map_err(|_| ())?;
                super::INSTALL_POSITION.store(verification.checked(), Ordering::Release);
                if result.is_some() {
                    return Ok(result);
                }
                Phase::Verify(verification)
            }
        });
        Ok(None)
    }
    pub fn step(&mut self, storage: &mut Storage) -> bool {
        if super::CANCELLED.load(Ordering::Acquire) {
            self.cancel(storage, 6);
            return false;
        }
        match self.advance(storage) {
            Ok(Some(_)) => {
                self.cancel(storage, 4);
                true
            }
            Ok(None) => false,
            Err(()) => {
                self.cancel(storage, 5);
                false
            }
        }
    }
}
