//! Flash file systems behind `mosaico-fs`.
//!
//! - `/system`: the official LittleFS partition (`system`, subtype 0x83),
//!   mounted read-only. It carries the factory Lua apps, scripts and fonts;
//!   the factory firmware treats it as a seed, so it is never written.
//! - `/data`: the strict A/B layout's named partition, or the positively
//!   identified legacy main_app tail. Unknown tables never select a writable region.
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

use esp_storage::ll;
use log::{info, warn};
use mosaico_fs::{global, BlockDevice, Errno, Fs, Geometry, Vfs};

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
        unsafe { ll::spiflash_read(addr, buf.as_mut_ptr().cast(), buf.len() as u32) }.map_err(
            |rc| {
                warn!("fs: flash read @{addr:#x}+{}: rc {rc}", buf.len());
                Errno::IO
            },
        )
    }

    fn unlock(&mut self) -> Result<(), Errno> {
        if !self.unlocked {
            // SAFETY: plain ROM call.
            unsafe { ll::spiflash_unlock() }.map_err(|rc| {
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
        unsafe { ll::spiflash_write(addr, data.as_ptr().cast(), data.len() as u32) }.map_err(|rc| {
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
        unsafe { ll::spiflash_erase_sector(addr / BLOCK) }.map_err(|rc| {
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
impl mosaico_s31::ota::Flash for OtaFlash {
    fn read(&mut self, a: u32, b: &mut [u8]) -> mosaico_s31::ota::Result<()> {
        read_bounced(a, b).map_err(|_| mosaico_s31::ota::Error::Flash)
    }
    fn write(&mut self, a: u32, b: &[u8]) -> mosaico_s31::ota::Result<()> {
        write_flash(a, b).map_err(|_| mosaico_s31::ota::Error::Flash)
    }
    fn erase(&mut self, a: u32) -> mosaico_s31::ota::Result<()> {
        erase_flash(a).map_err(|_| mosaico_s31::ota::Error::Flash)
    }
}
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static SYSTEM_MOUNTED: AtomicBool = AtomicBool::new(false);
static DATA_MOUNTED: AtomicBool = AtomicBool::new(false);
pub fn mounted() -> (bool, bool) {
    (
        SYSTEM_MOUNTED.load(Ordering::Acquire),
        DATA_MOUNTED.load(Ordering::Acquire),
    )
}
pub fn layout() -> mosaico_s31::ota::Result<mosaico_s31::ota::layout::Layout> {
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(mosaico_s31::ota::Error::Unavailable);
    }
    let mut table = [0; PARTITION_TABLE_LEN];
    read_bounced(PARTITION_TABLE, &mut table).map_err(|_| mosaico_s31::ota::Error::Flash)?;
    let capacity = {
        let _guard = flash_guard().map_err(|_| mosaico_s31::ota::Error::Flash)?;
        shared().0.size
    };
    mosaico_s31::ota::layout::parse(&table, capacity)
}
/// Read and verify the A/B table with fixed stack storage, before allocator setup.
pub fn early_ab() -> mosaico_s31::ota::Result<()> {
    if !INITIALIZED.load(Ordering::Acquire) {
        return Err(mosaico_s31::ota::Error::Unavailable);
    }
    let mut table = [0; PARTITION_TABLE_LEN];
    read_bounced(PARTITION_TABLE, &mut table).map_err(|_| mosaico_s31::ota::Error::Flash)?;
    let capacity = {
        let _guard = flash_guard().map_err(|_| mosaico_s31::ota::Error::Flash)?;
        shared().0.size
    };
    mosaico_s31::ota::layout::verify_ab_early(&table, capacity)
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
    let data = layout.data;
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
    match data {
        Some((base, len)) => {
            let blocks = len / BLOCK;
            let dev = FlashRegion {
                base,
                blocks,
                writable: true,
            };
            match Fs::mount_or_format(Box::new(dev), Geometry::flash(blocks)) {
                Ok((fs, formatted)) => {
                    vfs.mount("data", fs);
                    DATA_MOUNTED.store(true, Ordering::Release);
                    let how = if formatted { " (formatted)" } else { "" };
                    info!("fs: /data rw @{base:#x} {} KiB{how}", len / 1024);
                }
                Err(e) => warn!("fs: /data mount failed: {e}"),
            }
        }
        _ => warn!("fs: no room for /data in main_app"),
    }
    if let Err(e) = mosaico_s31::hub::uninstall::recover(&mut vfs) {
        warn!("fs: app recovery incomplete: {e}; pending identities remain blocked");
    }
    global::install(vfs);
    let _ = global::with(|v| {
        for dir in ["/system", "/data"] {
            if let Ok(s) = v.space(dir) {
                info!("fs: {dir} {}/{} blocks used", s.blocks - s.free, s.blocks);
            }
        }
        v.create_dir_all("/data/apps")
    });
}
