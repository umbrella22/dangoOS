//! Flash file systems behind `mosaico-fs`.
//!
//! - `/system`: the official LittleFS partition (`system`, subtype 0x83),
//!   mounted read-only. It carries the factory Lua apps, scripts and fonts;
//!   the factory firmware treats it as a seed, so it is never written.
//! - `/data`: our own LittleFS in the unused tail of `main_app`
//!   ([`DATA_SIZE`] bytes ending at the partition end). Pushed apps and the
//!   `storage` Lua module live here. It is formatted on first boot.
//!
//! All flash access goes through the installed VFS, whose busy lock
//! serialises callers. Reads and writes bounce through a static internal-RAM
//! buffer: littlefs caches can live in PSRAM, which the ROM flash routines
//! must not touch while the cache is disabled.
//!
//! The ROM routines are called through `esp_storage::ll` (each call in a
//! critical section) rather than `FlashStorage`: on the ESP32-S31 its flash
//! size detection does not give a usable size, so every bounds-checked read
//! failed. The size comes from the bootloader image header instead.

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use esp_storage::{ll, FlashStorage};
use log::{info, warn};
use mosaico_fs::{global, BlockDevice, Errno, Fs, Geometry, Kind, Vfs};

/// Size of `/data`; the firmware image must end below `main_app.end - DATA_SIZE`
/// (`scripts/flash-app.sh` checks it).
pub const DATA_SIZE: u32 = 2 * 1024 * 1024;
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

// SAFETY: only reached from block-device callbacks, which run under the VFS
// busy lock (one caller at a time), and from `init` before the VFS exists.
unsafe impl Sync for Shared {}

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
fn read_bounced(addr: u32, out: &mut [u8]) -> Result<(), Errno> {
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

/// `(label, type, subtype, offset, size)` of each partition table entry.
fn partitions(table: &[u8]) -> impl Iterator<Item = (&str, u8, u8, u32, u32)> {
    table
        .chunks_exact(32)
        .take_while(|e| e[0] == 0xAA && e[1] == 0x50)
        .map(|e| {
            let word = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
            let label = &e[12..28];
            let end = label.iter().position(|&b| b == 0).unwrap_or(label.len());
            let label = core::str::from_utf8(&label[..end]).unwrap_or("");
            (label, e[2], e[3], word(4), word(8))
        })
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
        let (flash, _) = shared();
        flash.erase_sector(addr)
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

/// Let the console pump flush the last log line before the next risky step.
async fn step(what: &str) {
    info!("fs: {what}");
    embassy_time::Timer::after_millis(30).await;
}

/// Read the partition table, mount `/system` and `/data`, and install the VFS.
/// Failures are logged; the firmware runs on without the missing mount.
pub async fn init(flash: esp_hal::peripherals::FLASH<'static>) {
    step("opening flash").await;
    // `FlashStorage` only for its size guess, logged for comparison.
    let guess = FlashStorage::new(flash).capacity();
    // SAFETY: no block device exists yet. The size is provisional: enough
    // for the bootloader header and the partition table.
    unsafe {
        *SHARED.flash.get() = Some(Nor {
            size: 0x10000,
            unlocked: false,
        })
    };
    let mut header_size = None;
    for at in [0u32, 0x2000] {
        let mut header = [0u8; 32];
        if read_bounced(at, &mut header).is_ok() {
            info!("fs: flash @{at:#x}: {:02x?}", &header[..8]);
            header_size = header_size.or(header_flash_size(&header));
        }
    }
    step("reading the partition table").await;
    let mut table = [0u8; PARTITION_TABLE_LEN];
    if let Err(e) = read_bounced(PARTITION_TABLE, &mut table) {
        warn!("fs: partition table unreadable: {e}");
    }
    info!("fs: partition table: {:02x?}", &table[..16]);
    // Without a header, the end of the last partition bounds the chip.
    let table_end = partitions(&table)
        .map(|(_, _, _, off, len)| off.saturating_add(len))
        .max()
        .map(|end| end.next_power_of_two());
    info!(
        "fs: flash size: header {} KiB, partitions {} KiB, esp-storage {} KiB",
        header_size.unwrap_or(0) / 1024,
        table_end.unwrap_or(0) / 1024,
        guess / 1024
    );
    let Some(size) = header_size
        .or(table_end)
        .or((guess >= 0x10_0000).then_some(guess as u32))
    else {
        warn!("fs: flash size unknown; no file systems");
        global::install(Vfs::new());
        return;
    };
    shared().0.size = size;

    let (mut system, mut main_app) = (None, None);
    for (label, kind, subtype, off, len) in partitions(&table) {
        match (label, kind, subtype) {
            // Data / LittleFS.
            ("system", 0x01, 0x83) => system = Some((off, len)),
            ("main_app", 0x00, _) => main_app = Some((off, len)),
            _ => {}
        }
    }

    let mut vfs = Vfs::new();
    if let Some((base, len)) = system {
        step("mounting /system").await;
        let dev = FlashRegion {
            base,
            blocks: len / BLOCK,
            writable: false,
        };
        match Fs::mount(Box::new(dev), Geometry::flash(len / BLOCK), true) {
            Ok(fs) => {
                vfs.mount("system", fs);
                info!("fs: /system ro @{base:#x} {} KiB", len / 1024);
            }
            Err(e) => warn!("fs: /system mount failed: {e}"),
        }
    } else {
        warn!("fs: no system partition");
    }
    match main_app {
        Some((off, len)) if len > DATA_SIZE => {
            let base = off + len - DATA_SIZE;
            let blocks = DATA_SIZE / BLOCK;
            let dev = FlashRegion {
                base,
                blocks,
                writable: true,
            };
            step("mounting /data").await;
            match Fs::mount_or_format(Box::new(dev), Geometry::flash(blocks)) {
                Ok((fs, formatted)) => {
                    vfs.mount("data", fs);
                    let how = if formatted { " (formatted)" } else { "" };
                    info!("fs: /data rw @{base:#x} {} KiB{how}", DATA_SIZE / 1024);
                }
                Err(e) => warn!("fs: /data mount failed: {e}"),
            }
        }
        _ => warn!("fs: no room for /data in main_app"),
    }
    global::install(vfs);
    step("listing").await;

    let _ = global::with(|v| {
        for dir in ["/system", "/data"] {
            if let Ok(s) = v.space(dir) {
                info!("fs: {dir} {}/{} blocks used", s.blocks - s.free, s.blocks);
            }
        }
        let _ = v.create_dir_all("/data/apps");
        for root in ["/data/apps", "/system/apps"] {
            if let Ok(entries) = v.read_dir(root) {
                let mut names = alloc::string::String::new();
                for e in entries.iter().filter(|e| e.meta.kind == Kind::Dir) {
                    names.push(' ');
                    names.push_str(&e.name);
                }
                info!("fs: {root}:{names}");
            }
        }
        Ok(())
    });
}
