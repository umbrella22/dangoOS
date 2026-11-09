//! Explicit main-image installation. This module is separate from read-only
//! media dispatch; callers must deliberately create an installation session.
use boot_contract::{self as boot, Flash, State, MAIN_BASE, MAIN_BYTES, SECTOR};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest {
    pub layout: [u8; 8],
    pub bytes: u32,
    pub sha256: [u8; 32],
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Manifest,
    Offset,
    Length,
    Hash,
    Readback,
    Image,
    Flash(E),
    Journal(boot::Error<E>),
}
pub struct Installer {
    manifest: Manifest,
    received: u32,
    erased: u32,
    digest: Sha256,
    image: boot::image::ImageVerifier,
    failed: bool,
}
/// A caller yields between readback sectors and can drop this state on cancel.
pub struct Verification {
    manifest: Manifest,
    checked: u32,
    digest: Sha256,
    failed: bool,
}
/// Validate a complete staged file before the single main image is erased.
pub struct Preflight {
    manifest: Manifest,
    received: u32,
    digest: Sha256,
    image: boot::image::ImageVerifier,
}
impl Preflight {
    pub fn new<E>(manifest: Manifest) -> Result<Self, Error<E>> {
        if manifest.layout != *boot::LAYOUT || manifest.bytes < 24 || manifest.bytes > MAIN_BYTES {
            return Err(Error::Manifest);
        }
        Ok(Self {
            manifest,
            received: 0,
            digest: Sha256::new(),
            image: boot::image::ImageVerifier::new(manifest.bytes),
        })
    }
    pub fn received(&self) -> u32 {
        self.received
    }
    pub fn feed<E>(&mut self, bytes: &[u8]) -> Result<(), Error<E>> {
        let n = u32::try_from(bytes.len()).map_err(|_| Error::Length)?;
        if n == 0
            || self
                .received
                .checked_add(n)
                .is_none_or(|end| end > self.manifest.bytes)
        {
            return Err(Error::Length);
        }
        self.image.feed(bytes).map_err(|_| Error::Image)?;
        self.digest.update(bytes);
        self.received += n;
        Ok(())
    }
    pub fn finish<E>(self) -> Result<Manifest, Error<E>> {
        if self.received != self.manifest.bytes {
            return Err(Error::Length);
        }
        self.image.finish().map_err(|_| Error::Image)?;
        if self.digest.finalize().as_slice() != self.manifest.sha256 {
            return Err(Error::Hash);
        }
        Ok(self.manifest)
    }
}
impl Installer {
    /// Commit Installing before touching any byte of the only main image.
    pub fn begin<F: Flash>(flash: &mut F, manifest: Manifest) -> Result<Self, Error<F::Error>> {
        if manifest.layout != *boot::LAYOUT || manifest.bytes < 24 || manifest.bytes > MAIN_BYTES {
            return Err(Error::Manifest);
        }
        boot::append(flash, State::Installing, manifest.bytes, manifest.sha256)
            .map_err(Error::Journal)?;
        Ok(Self {
            manifest,
            received: 0,
            erased: 0,
            digest: Sha256::new(),
            image: boot::image::ImageVerifier::new(manifest.bytes),
            failed: false,
        })
    }
    pub fn received(&self) -> u32 {
        self.received
    }
    /// Offsets are relative to main. Sequential, word-aligned transfers make
    /// each erase occur before that sector's first program. A repeated last
    /// transfer is acknowledged only after comparing its bytes with NOR.
    pub fn push<F: Flash>(
        &mut self,
        flash: &mut F,
        offset: u32,
        bytes: &[u8],
    ) -> Result<(), Error<F::Error>> {
        let length = u32::try_from(bytes.len()).map_err(|_| Error::Length)?;
        if self.failed {
            return Err(Error::Image);
        }
        let end = offset.checked_add(length).ok_or(Error::Length)?;
        if bytes.is_empty()
            || bytes.len() > 4096
            || offset % 4 != 0
            || end > self.manifest.bytes
            || (length % 4 != 0 && end != self.manifest.bytes)
        {
            return Err(Error::Length);
        }
        if offset < self.received {
            if end > self.received {
                return Err(Error::Offset);
            }
            let mut verify = [0; 4096];
            flash
                .read(MAIN_BASE + offset, &mut verify[..bytes.len()])
                .map_err(Error::Flash)?;
            return if &verify[..bytes.len()] == bytes {
                Ok(())
            } else {
                Err(Error::Readback)
            };
        }
        if offset != self.received {
            return Err(Error::Offset);
        }
        if offset == 0 && bytes[0] != 0xe9 {
            return Err(Error::Manifest);
        }
        if self.image.feed(bytes).is_err() {
            self.failed = true;
            return Err(Error::Image);
        }
        while self.erased < end {
            if let Err(error) = flash.erase(MAIN_BASE + self.erased) {
                self.failed = true;
                return Err(Error::Flash(error));
            }
            self.erased += SECTOR;
        }
        let aligned = (bytes.len() + 3) & !3;
        let mut staging = [255; 4096];
        staging[..bytes.len()].copy_from_slice(bytes);
        if let Err(error) = flash.program(MAIN_BASE + offset, &staging[..aligned]) {
            self.failed = true;
            return Err(Error::Flash(error));
        }
        self.digest.update(bytes);
        self.received = end;
        Ok(())
    }
    pub fn start_verify<E>(self) -> Result<Verification, Error<E>> {
        if self.received != self.manifest.bytes {
            return Err(Error::Length);
        }
        if self.failed || self.image.finish().is_err() {
            return Err(Error::Image);
        }
        if self.digest.finalize().as_slice() != self.manifest.sha256 {
            return Err(Error::Hash);
        }
        Ok(Verification {
            manifest: self.manifest,
            checked: 0,
            digest: Sha256::new(),
            failed: false,
        })
    }
    /// Synchronous convenience for host callers; firmware yields between steps.
    pub fn finish<F: Flash>(self, flash: &mut F) -> Result<boot::Record, Error<F::Error>> {
        let mut verification = self.start_verify()?;
        loop {
            if let Some(record) = verification.step(flash)? {
                return Ok(record);
            }
        }
    }
}
impl Verification {
    pub fn checked(&self) -> u32 {
        self.checked
    }
    /// Read at most one sector. Trial is committed only on the final good hash.
    pub fn step<F: Flash>(
        &mut self,
        flash: &mut F,
    ) -> Result<Option<boot::Record>, Error<F::Error>> {
        if self.failed || self.checked >= self.manifest.bytes {
            return Err(Error::Offset);
        }
        let mut buffer = [0; 4096];
        let n = (self.manifest.bytes - self.checked).min(buffer.len() as u32) as usize;
        if let Err(error) = flash.read(MAIN_BASE + self.checked, &mut buffer[..n]) {
            self.failed = true;
            return Err(Error::Flash(error));
        }
        self.digest.update(&buffer[..n]);
        self.checked += n as u32;
        if self.checked < self.manifest.bytes {
            return Ok(None);
        }
        if self.digest.clone().finalize().as_slice() != self.manifest.sha256 {
            self.failed = true;
            return Err(Error::Readback);
        }
        boot::append(
            flash,
            State::Trial,
            self.manifest.bytes,
            self.manifest.sha256,
        )
        .map(Some)
        .map_err(Error::Journal)
    }
}
