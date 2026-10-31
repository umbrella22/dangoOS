//! Flash file systems behind `fs`.
//!
//! - `/system`: the LittleFS NOR partition (`system`, subtype 0x83), mounted
//!   read-only. It carries packaged Lua apps and the complete default skin.
//! - `/data`: the SPI NAND volume, installed by `nand::volume` after NOR init;
//!   `/nand` aliases that same filesystem. There is no writable NOR data tail.
//!
//! VFS and OTA ROM accesses share FlashGuard, including the static internal-RAM
//! bounce buffer. LittleFS caches and OTA input may live in PSRAM, which the ROM
//! routines must not touch while the cache is disabled.
//!
//! The ROM routines are called through `esp_storage::ll` (each call in a
//! critical section) rather than `FlashStorage`: on the ESP32-S31 its flash
//! size detection does not give a usable size, so every bounds-checked read
//! failed. The size comes from the bootloader image header (at 0x2000 on the
//! S31, 0 on older chips). Unknown headers/layouts are rejected; partition
//! endpoints are not used to guess the capacity of an OTA-capable device.

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use ::fs::{global, BlockDevice, Errno, Fs, Geometry, Vfs};
use esp_storage::ll;
use log::{info, warn};

/// Enable the shared core rendezvous before starting Core1. NOR ROM callers
/// retain FlashGuard; the rendezvous also excludes concurrent clock changes.
#[cfg(feature = "dual-core-ui")]
pub fn enable_multicore() {
    crate::platform::runtime::cores::enable_multicore();
}

fn rom_access<R>(f: impl FnOnce() -> R) -> R {
    // ROM routines take the global section themselves. Take it before the
    // shared rendezvous so a parked sibling cannot own that section. The
    // clock backend takes its clock-tree lock before the same rendezvous;
    // ROM routines never take the clock-tree lock.
    critical_section::with(|_| {
        // SAFETY: f is the existing ROM operation; the rendezvous only adds
        // exclusion against a CPU clock transition. No new OS locks run here.
        unsafe { crate::platform::runtime::cores::with_other_parked(f) }
    })
}

const BLOCK: u32 = 4096;

/// The partition table: offset and the most it can hold.
const PARTITION_TABLE: u32 = 0x8000;
const PARTITION_TABLE_LEN: usize = 3072;

/// The SPI NOR chip through the ROM routines.
struct Nor {
    size: u32,
    unlocked: bool,
}

impl Nor {
    fn check(&self, addr: u32, len: usize) -> Result<(), Errno> {
        if addr % 4 != 0 || len % 4 != 0 || addr as u64 + len as u64 > self.size as u64 {
            return Err(Errno::INVAL);
        }
        Ok(())
    }

    /// `buf` must be the bounce buffer (word aligned, internal RAM).
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Errno> {
        self.check(addr, buf.len())?;
        // SAFETY: bounds checked; `buf` is word aligned (see `Shared`).
        rom_access(|| unsafe { ll::spiflash_read(addr, buf.as_mut_ptr().cast(), buf.len() as u32) })
            .map_err(|rc| {
                warn!("fs: flash read @{addr:#x}+{}: rc {rc}", buf.len());
                Errno::IO
            })
    }

    fn unlock(&mut self) -> Result<(), Errno> {
        if !self.unlocked {
            // SAFETY: plain ROM call.
            rom_access(|| unsafe { ll::spiflash_unlock() }).map_err(|rc| {
                warn!("fs: flash unlock: rc {rc}");
                Errno::IO
            })?;
            self.unlocked = true;
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), Errno> {
        self.check(addr, data.len())?;
        self.unlock()?;
        // SAFETY: as in `read`.
        rom_access(|| unsafe { ll::spiflash_write(addr, data.as_ptr().cast(), data.len() as u32) })
            .map_err(|rc| {
                warn!("fs: flash write @{addr:#x}+{}: rc {rc}", data.len());
                Errno::IO
            })
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), Errno> {
        self.check(addr, BLOCK as usize)?;
        if addr % BLOCK != 0 {
            return Err(Errno::INVAL);
        }
        self.unlock()?;
        // SAFETY: bounds checked.
        rom_access(|| unsafe { ll::spiflash_erase_sector(addr / BLOCK) }).map_err(|rc| {
            warn!("fs: flash erase @{addr:#x}: rc {rc}");
            Errno::IO
        })
    }
}

struct Shared {
    flash: UnsafeCell<Option<Nor>>,
    bounce: UnsafeCell<[u32; 128]>,
}

// SAFETY: ROM operations and bounce storage are serialized by FlashGuard across
// VFS callbacks, OTA and initialization. init_flash runs before tasks exist.
unsafe impl Sync for Shared {}

// Shared with OTA; every ROM operation and bounce-buffer borrow is serialized.
static FLASH_BUSY: AtomicBool = AtomicBool::new(false);
struct FlashGuard;
impl Drop for FlashGuard {
    fn drop(&mut self) {
        FLASH_BUSY.store(false, Ordering::Release);
    }
}
fn flash_guard() -> Result<FlashGuard, Errno> {
    FLASH_BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| FlashGuard)
        .map_err(|_| Errno::BUSY)
}

static SHARED: Shared = Shared {
    flash: UnsafeCell::new(None),
    bounce: UnsafeCell::new([0; 128]),
};

#[allow(clippy::mut_from_ref)]
fn shared() -> (&'static mut Nor, &'static mut [u8; 512]) {
    // SAFETY: see `Shared`; `init` stores the flash before any device exists.
    unsafe {
        let flash = (*SHARED.flash.get())
            .as_mut()
            .expect("flash not initialised");
        let bounce = &mut *SHARED.bounce.get().cast::<[u8; 512]>();
        (flash, bounce)
    }
}

/// Read `out.len()` bytes at `addr` (word aligned) through the bounce buffer.
pub fn read_bounced(addr: u32, out: &mut [u8]) -> Result<(), Errno> {
    let _guard = flash_guard()?;
    let (flash, bounce) = shared();
    let mut a = addr;
    for chunk in out.chunks_mut(bounce.len()) {
        let tmp = &mut bounce[..chunk.len()];
        flash.read(a, tmp)?;
        chunk.copy_from_slice(tmp);
        a += chunk.len() as u32;
    }
    Ok(())
}

/// Flash size from the bootloader image header at 0 (byte 3, high nibble).
fn header_flash_size(header: &[u8]) -> Option<u32> {
    if header.first() != Some(&0xE9) {
        return None;
    }
    let mb = match header[3] >> 4 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 16,
        5 => 32,
        6 => 64,
        7 => 128,
        _ => return None,
    };
    Some(mb * 1024 * 1024)
}

/// A block-aligned window of the SPI flash.
struct FlashRegion {
    base: u32,
    blocks: u32,
    writable: bool,
}

impl FlashRegion {
    fn addr(&self, block: u32, off: u32, len: usize) -> Result<u32, Errno> {
        if block >= self.blocks
            || off as usize + len > BLOCK as usize
            || off % 4 != 0
            || len % 4 != 0
        {
            return Err(Errno::INVAL);
        }
        Ok(self.base + block * BLOCK + off)
    }
}

impl BlockDevice for FlashRegion {
    fn read(&mut self, block: u32, off: u32, buf: &mut [u8]) -> Result<(), Errno> {
        let addr = self.addr(block, off, buf.len())?;
        read_bounced(addr, buf)
    }

    fn prog(&mut self, block: u32, off: u32, data: &[u8]) -> Result<(), Errno> {
        if !self.writable {
            return Err(Errno::ROFS);
        }
        let mut addr = self.addr(block, off, data.len())?;
        let _guard = flash_guard()?;
        let (flash, bounce) = shared();
        for chunk in data.chunks(bounce.len()) {
            let tmp = &mut bounce[..chunk.len()];
            tmp.copy_from_slice(chunk);
            flash.write(addr, tmp)?;
            addr += chunk.len() as u32;
        }
        Ok(())
    }

    fn erase(&mut self, block: u32) -> Result<(), Errno> {
        if !self.writable {
            return Err(Errno::ROFS);
        }
        let addr = self.addr(block, 0, 0)?;
        erase_flash(addr)
    }
}

pub fn write_flash(addr: u32, data: &[u8]) -> Result<(), Errno> {
    let _guard = flash_guard()?;
    let (flash, bounce) = shared();
    let mut at = addr;
    for chunk in data.chunks(512) {
        bounce[..chunk.len()].copy_from_slice(chunk);
        flash.write(at, &bounce[..chunk.len()])?;
        at += chunk.len() as u32;
    }
    Ok(())
}
pub fn erase_flash(addr: u32) -> Result<(), Errno> {
    let _guard = flash_guard()?;
    shared().0.erase_sector(addr)
}
pub struct OtaFlash;
impl crate::services::update::ota::Flash for OtaFlash {
    fn read(&mut self, a: u32, b: &mut [u8]) -> crate::services::update::ota::Result<()> {
        read_bounced(a, b).map_err(|_| crate::services::update::ota::Error::Flash)
    }
    fn write(&mut self, a: u32, b: &[u8]) -> crate::services::update::ota::Result<()> {
        write_flash(a, b).map_err(|_| crate::services::update::ota::Error::Flash)
    }
    fn erase(&mut self, a: u32) -> crate::services::update::ota::Result<()> {
        erase_flash(a).map_err(|_| crate::services::update::ota::Error::Flash)
    }
}
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static SYSTEM_MOUNTED: AtomicBool = AtomicBool::new(false);
pub fn mounted() -> (bool, bool) {
    (
        SYSTEM_MOUNTED.load(Ordering::Acquire),
        crate::platform::storage::nand::mounted(),
    )
}
pub fn layout() -> crate::services::update::ota::Result<crate::services::update::ota::layout::Layout>
{
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(crate::services::update::ota::Error::Unavailable);
    }
    let mut table = [0; PARTITION_TABLE_LEN];
    read_bounced(PARTITION_TABLE, &mut table)
        .map_err(|_| crate::services::update::ota::Error::Flash)?;
    let capacity = {
        let _guard = flash_guard().map_err(|_| crate::services::update::ota::Error::Flash)?;
        shared().0.size
    };
    crate::services::update::ota::layout::parse(&table, capacity)
}
/// Read and verify the A/B table with fixed stack storage, before allocator setup.
pub fn early_ab() -> crate::services::update::ota::Result<()> {
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(crate::services::update::ota::Error::Unavailable);
    }
    let mut table = [0; PARTITION_TABLE_LEN];
    read_bounced(PARTITION_TABLE, &mut table)
        .map_err(|_| crate::services::update::ota::Error::Flash)?;
    let capacity = {
        let _guard = flash_guard().map_err(|_| crate::services::update::ota::Error::Flash)?;
        shared().0.size
    };
    crate::services::update::ota::layout::verify_ab_early(&table, capacity)
}
/// Initializes ROM flash access without allocation, before PSRAM/panel/filesystems.
pub fn init_flash() {
    if INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        *SHARED.flash.get() = Some(Nor {
            size: 0x10000,
            unlocked: false,
        });
    }
    let mut size = None;
    for at in [0u32, 0x2000] {
        let mut h = [0; 32];
        if read_bounced(at, &mut h).is_ok() {
            size = size.or(header_flash_size(&h));
        }
    }
    if let Some(size) = size {
        shared().0.size = size;
        INITIALIZED.store(true, Ordering::Release);
    }
}

static READY: AtomicBool = AtomicBool::new(false);

/// `init` has finished (or was skipped): the app registry can be scanned.
pub fn mark_ready() {
    READY.store(true, Ordering::Release);
}

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Read the partition table, mount `/system` and `/data`, and install the VFS.
/// Failures are logged; the firmware runs on without the missing mount.
///
/// Takes the `FLASH` peripheral so nothing else drives the chip meanwhile.
pub fn init(_flash: esp_hal::peripherals::FLASH<'static>) {
    init_flash();
    let layout = match layout() {
        Ok(layout) => layout,
        Err(e) => {
            warn!("fs: unsafe/unknown partition table: {e}");
            global::install(Vfs::new());
            return;
        }
    };
    let system = layout
        .entries
        .iter()
        .find(|p| p.label == "system" && p.kind == 1 && p.subtype == 0x83)
        .map(|p| (p.offset, p.size));
    let mut vfs = Vfs::new();
    if let Some((base, len)) = system {
        let dev = FlashRegion {
            base,
            blocks: len / BLOCK,
            writable: false,
        };
        match Fs::mount(Box::new(dev), Geometry::flash(len / BLOCK), true) {
            Ok(fs) => {
                vfs.mount("system", fs);
                SYSTEM_MOUNTED.store(true, Ordering::Release);
                info!("fs: /system ro @{base:#x} {} KiB", len / 1024);
            }
            Err(e) => warn!("fs: /system mount failed: {e}"),
        }
    } else {
        warn!("fs: no system partition");
    }
    global::install(vfs);
}
