//! No erase/program functions exist in the Recovery media adapters.
use crate::{
    protocol::{read_range, Status},
    readonly::{ReadOnlyBlock, ReadOnlyNand},
    server::{Media, Request, Volume},
};
use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use board_s31::nand as chip;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use fs::{
    nand::{Ftl, Nand},
    BlockDevice, Errno, Fs, Geometry, Kind, OpenFlags, Whence,
};
use serde_json::json;

// Physical capacity of the S31 board, independent of damaged partition records.
pub const NOR_BYTES: u32 = 16 * 1024 * 1024;
const SYSTEM_BASE: u32 = 0xe10000;
const SYSTEM_BYTES: u32 = NOR_BYTES - SYSTEM_BASE;

pub struct Nor;
impl Nor {
    pub fn read(offset: u32, out: &mut [u8]) -> Result<(), Errno> {
        if u64::from(offset) + out.len() as u64 > u64::from(NOR_BYTES) {
            return Err(Errno::INVAL);
        }
        // ROM SPI routines require word alignment and internal storage. This
        // bounce buffer lives on the Core0 internal stack, never PSRAM.
        let mut bounce = [0u32; 128];
        let mut cursor = offset;
        let mut rest = out;
        while !rest.is_empty() {
            let address = cursor & !3;
            let skip = (cursor - address) as usize;
            let length = rest.len().min(512 - skip);
            let read_len = (skip + length + 3) & !3;
            let result = critical_section::with(|_| unsafe {
                esp_storage::ll::spiflash_read(address, bounce.as_mut_ptr().cast(), read_len as u32)
            });
            if result.is_err() {
                return Err(Errno::IO);
            }
            let bytes =
                unsafe { core::slice::from_raw_parts(bounce.as_ptr().cast::<u8>(), read_len) };
            rest[..length].copy_from_slice(&bytes[skip..skip + length]);
            rest = &mut rest[length..];
            cursor += length as u32;
        }
        Ok(())
    }
}
struct System;
impl BlockDevice for System {
    fn read(&mut self, block: u32, offset: u32, out: &mut [u8]) -> Result<(), Errno> {
        if block >= SYSTEM_BYTES / 4096 || u64::from(offset) + out.len() as u64 > 4096 {
            return Err(Errno::INVAL);
        }
        Nor::read(SYSTEM_BASE + block * 4096 + offset, out)
    }
    fn prog(&mut self, _: u32, _: u32, _: &[u8]) -> Result<(), Errno> {
        Err(Errno::ROFS)
    }
    fn erase(&mut self, _: u32) -> Result<(), Errno> {
        Err(Errno::ROFS)
    }
}
struct Physical;
fn chip_error(e: chip::Error) -> fs::nand::Error {
    if e == chip::Error::Uncorrectable {
        fs::nand::Error::Uncorrectable
    } else {
        fs::nand::Error::Io
    }
}
impl Nand for Physical {
    fn blocks(&self) -> u32 {
        chip::BLOCKS
    }
    fn is_bad(&mut self, b: u32) -> Result<bool, fs::nand::Error> {
        chip::with(|n| n.is_bad(b)).map_err(chip_error)
    }
    fn is_free(&mut self, p: u32) -> Result<bool, fs::nand::Error> {
        chip::with(|n| n.is_free(p)).map_err(chip_error)
    }
    fn read(&mut self, p: u32, o: usize, d: &mut [u8]) -> Result<(), fs::nand::Error> {
        chip::with(|n| n.read_data(p, o, d))
            .map(|_| ())
            .map_err(chip_error)
    }
    fn mark_bad(&mut self, _: u32) -> Result<(), fs::nand::Error> {
        Err(fs::nand::Error::Io)
    }
    fn erase(&mut self, _: u32) -> Result<(), fs::nand::Error> {
        Err(fs::nand::Error::Io)
    }
    fn program(&mut self, _: u32, _: &[u8]) -> Result<(), fs::nand::Error> {
        Err(fs::nand::Error::Io)
    }
}
#[derive(Clone)]
struct Logical(Arc<Mutex<CriticalSectionRawMutex, Ftl>>);
impl BlockDevice for Logical {
    fn read(&mut self, b: u32, o: u32, d: &mut [u8]) -> Result<(), Errno> {
        self.0.try_lock().map_err(|_| Errno::BUSY)?.read(b, o, d)
    }
    fn prog(&mut self, _: u32, _: u32, _: &[u8]) -> Result<(), Errno> {
        Err(Errno::ROFS)
    }
    fn erase(&mut self, _: u32) -> Result<(), Errno> {
        Err(Errno::ROFS)
    }
}
pub struct Storage {
    staged: Option<fs::lfs::File>,
    ftl: Option<Logical>,
    data: Option<Fs>,
    system: Option<Fs>,
}
impl Default for Storage {
    fn default() -> Self {
        Self {
            staged: None,
            ftl: None,
            data: None,
            system: None,
        }
    }
}
fn status(e: Errno) -> Status {
    match e {
        Errno::NOENT => Status::NotFound,
        Errno::INVAL => Status::Invalid,
        Errno::ROFS => Status::ReadOnly,
        _ => Status::Io,
    }
}
fn heaps() -> serde_json::Value {
    let stats = esp_alloc::HEAP.stats();
    let regions: Vec<_> = stats
        .region_stats
        .iter()
        .flatten()
        .map(|r| {
            json!({"external":r.capabilities.contains(esp_alloc::MemoryCapability::External),
            "bytes":r.size,"used":r.used,"free":r.free,"largest":r.largest_free_block,
            "allocated_blocks":r.allocated_blocks,"free_blocks":r.free_blocks})
        })
        .collect();
    json!(regions)
}
impl Storage {
    pub fn open_staged(&mut self, expected_bytes: u32) -> Result<(), Errno> {
        self.close_staged()?;
        let fs = self.volume(Volume::Data)?;
        let meta = fs.stat(boot_contract::STAGED_IMAGE)?;
        if meta.kind != Kind::File || meta.size != expected_bytes {
            return Err(Errno::INVAL);
        }
        self.staged = Some(fs.open(boot_contract::STAGED_IMAGE, OpenFlags::READ)?);
        Ok(())
    }
    pub fn read_staged(&mut self, buffer: &mut [u8]) -> Result<usize, Errno> {
        let f = self.staged.as_mut().ok_or(Errno::INVAL)?;
        self.data.as_mut().ok_or(Errno::INVAL)?.read(f, buffer)
    }
    pub fn rewind_staged(&mut self) -> Result<(), Errno> {
        let f = self.staged.as_mut().ok_or(Errno::INVAL)?;
        self.data
            .as_mut()
            .ok_or(Errno::INVAL)?
            .seek(f, 0, Whence::Set)?;
        Ok(())
    }
    pub fn close_staged(&mut self) -> Result<(), Errno> {
        if let Some(file) = self.staged.take() {
            self.data.as_mut().ok_or(Errno::INVAL)?.close(file)?;
        }
        Ok(())
    }
    fn logical(&mut self) -> Result<&mut Logical, Errno> {
        if self.ftl.is_none() {
            let ftl = Ftl::resume(Box::new(ReadOnlyNand(Physical)))?;
            self.ftl = Some(Logical(Arc::new(Mutex::new(ftl))));
        }
        Ok(self.ftl.as_mut().unwrap())
    }
    fn volume(&mut self, volume: Volume) -> Result<&mut Fs, Errno> {
        match volume {
            Volume::Data => {
                if self.data.is_none() {
                    let logical = self.logical()?.clone();
                    let geometry = logical.0.try_lock().map_err(|_| Errno::BUSY)?.geometry();
                    self.data = Some(Fs::mount(Box::new(ReadOnlyBlock(logical)), geometry, true)?);
                }
                Ok(self.data.as_mut().unwrap())
            }
            Volume::System => {
                if self.system.is_none() {
                    self.system = Some(Fs::mount(
                        Box::new(ReadOnlyBlock(System)),
                        Geometry::flash(SYSTEM_BYTES / 4096),
                        true,
                    )?);
                }
                Ok(self.system.as_mut().unwrap())
            }
        }
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        let _ = self.close_staged();
    }
}
impl Media for Storage {
    fn execute(&mut self, r: Request<'_>) -> Result<Vec<u8>, Status> {
        let value = match r {
            Request::Hello => {
                json!({"protocol":"RCR1","mode":if super::WRITE_MODE.load(core::sync::atomic::Ordering::Acquire){"install"}else{"read-only"},"board":"mosaico-s31","identity":esp_hal::efuse::base_mac_address().as_bytes(),"version":env!("CARGO_PKG_VERSION"),
                "nor_bytes":NOR_BYTES,"nand":{"blocks":chip::BLOCKS,"pages_per_block":chip::PAGES_PER_BLOCK,"data_bytes":chip::DATA_BYTES,"spare_bytes":chip::SPARE_BYTES},
                "transfer_bytes":crate::protocol::PAYLOAD,"layout_required":false})
            }
            Request::Status => {
                let (inherited, splash_us, selected_us) = board_s31::boot_display::status();
                json!({"data_mounted":self.data.is_some(),"system_mounted":self.system.is_some(),"logical_ready":self.ftl.is_some(),
                "display":super::DISPLAY.load(core::sync::atomic::Ordering::Acquire),"touch":super::TOUCH.load(core::sync::atomic::Ordering::Acquire),
                "touch_events":super::TOUCH_EVENTS.load(core::sync::atomic::Ordering::Acquire),"cancelled":super::CANCELLED.load(core::sync::atomic::Ordering::Acquire),
                "intro_frames":super::INTRO_FRAMES.load(core::sync::atomic::Ordering::Acquire),"intro_ms":super::INTRO_MS.load(core::sync::atomic::Ordering::Acquire),
                "intro_decode_us":super::INTRO_DECODE_US.load(core::sync::atomic::Ordering::Acquire),"intro_draw_us":super::INTRO_DRAW_US.load(core::sync::atomic::Ordering::Acquire),"cpu_hz":esp_hal::clock::cpu_clock().as_hz(),
                "heap_free":esp_alloc::HEAP.free(),"heaps":heaps(),"read_only":!super::WRITE_MODE.load(core::sync::atomic::Ordering::Acquire),
                "boot_diagnostic":super::boot_diagnostic(),
                "boot_display":{"inherited":inherited,"splash_us":splash_us,"selected_us":selected_us},
                "install_requires_layout":"S31ONE03","install_phase":super::INSTALL_PHASE.load(core::sync::atomic::Ordering::Acquire),
                "install_position":super::INSTALL_POSITION.load(core::sync::atomic::Ordering::Acquire),"install_total":super::INSTALL_TOTAL.load(core::sync::atomic::Ordering::Acquire)})
            }
            Request::Nor { offset, length } => {
                read_range(offset as u64, length, NOR_BYTES as u64).map_err(|_| Status::Invalid)?;
                let mut out = vec![0; length];
                Nor::read(offset, &mut out).map_err(status)?;
                return Ok(out);
            }
            Request::NandPage { page, raw } => {
                let mut out = vec![0; 12 + chip::PAGE_BYTES];
                let s = chip::with(|n| n.read_recovery_page(page, raw, &mut out[12..])).map_err(
                    |e| match e {
                        chip::Error::PageRange => Status::Invalid,
                        chip::Error::Unavailable => Status::Unavailable,
                        _ => Status::Io,
                    },
                )?;
                out[..4].copy_from_slice(&page.to_le_bytes());
                out[4] = s.raw as u8;
                out[5] = s.status;
                out[6] = s.extended_status.unwrap_or(0);
                out[7] = s.extended_status.is_some() as u8;
                out[8] = s.corrected_min;
                out[9] = s.corrected_max;
                out[10] = s.uncorrectable as u8;
                return Ok(out);
            }
            Request::Mount(volume) => {
                let fs = self.volume(volume).map_err(status)?;
                let g = fs.geometry();
                json!({"read_only":fs.read_only(),"bytes":u64::from(g.block_size)*u64::from(g.block_count)})
            }
            Request::List {
                volume,
                index,
                path,
            } => {
                let entries = self
                    .volume(volume)
                    .map_err(status)?
                    .read_dir_page(path, index as usize, 1)
                    .map_err(status)?;
                if let Some(e) = entries.first() {
                    json!({"index":index,"name":e.name,"size":e.meta.size,"kind":if e.meta.kind==Kind::Dir {"directory"}else{"file"}})
                } else {
                    json!({"end":true,"index":index})
                }
            }
            Request::File {
                volume,
                offset,
                length,
                path,
            } => {
                let fs = self.volume(volume).map_err(status)?;
                let mut f = fs.open(path, OpenFlags::READ).map_err(status)?;
                let result = (|| {
                    let size = fs.size(&mut f)?;
                    if offset > size {
                        return Err(Errno::INVAL);
                    }
                    fs.seek(&mut f, offset as i64, Whence::Set)?;
                    let mut out = vec![0; 4 + length];
                    out[..4].copy_from_slice(&size.to_le_bytes());
                    let n = fs.read(&mut f, &mut out[4..])?;
                    out.truncate(4 + n);
                    Ok(out)
                })();
                let closed = fs.close(f);
                let out = result.map_err(status)?;
                closed.map_err(status)?;
                return Ok(out);
            }
            Request::Logical { offset, length } => {
                let ftl = self.logical().map_err(status)?;
                let g = ftl.0.try_lock().map_err(|_| Status::Io)?.geometry();
                read_range(
                    offset,
                    length,
                    u64::from(g.block_size) * u64::from(g.block_count),
                )
                .map_err(|_| Status::Invalid)?;
                let mut out = vec![0; length];
                let mut bounce = [0; 2048];
                let mut position = offset;
                let mut done = 0;
                while done < length {
                    let block = (position / 4096) as u32;
                    let off = (position % 4096) as u32 & !2047;
                    ftl.read(block, off, &mut bounce).map_err(status)?;
                    let skip = position as usize % 2048;
                    let n = (length - done).min(2048 - skip);
                    out[done..done + n].copy_from_slice(&bounce[skip..skip + n]);
                    done += n;
                    position += n as u64;
                }
                return Ok(out);
            }
        };
        serde_json::to_vec(&value).map_err(|_| Status::Io)
    }
}
