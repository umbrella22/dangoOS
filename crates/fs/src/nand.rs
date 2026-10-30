//! Dhara mapping for 2048-byte pages, 64 pages per physical erase block.
//! Logical erase trims sectors; sync commits data and map checkpoints.
//! A transport error poisons the instance until an explicit remount.
use crate::{BlockDevice, Errno, Geometry};
use alloc::boxed::Box;
use core::{ffi::c_void, ptr::NonNull};

pub const PAGE_SIZE: usize = 2048;
pub const PAGES_PER_BLOCK: u32 = 64;
/// Fixed for this on-media mapping: four GC steps per foreground write.
pub const GC_RATIO: u8 = 4;

mod cache;
use cache::ReadCache;

/// Stable logical geometry. Reserve 1/16 of physical blocks for bad blocks,
/// one journal block, one checkpoint page in sixteen, Dhara's GC reserve and
/// its eight-block recovery margin. Never expose the moving capacity estimate
/// as the filesystem's block count.
fn logical_sectors(blocks: u32) -> u32 {
    let pages = (blocks - blocks.div_ceil(16) - 1) * (PAGES_PER_BLOCK / 16 * 15);
    (pages - pages / (u32::from(GC_RATIO) + 1)).saturating_sub(8 * PAGES_PER_BLOCK) & !1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Only a completed, status-checked program or erase failure.
    BadBlock,
    Uncorrectable,
    /// Bus, timeout, mode/geometry or persistent bad-marker failure.
    Io,
}
pub trait Nand: Send {
    fn blocks(&self) -> u32;
    fn is_bad(&mut self, block: u32) -> Result<bool, Error>;
    fn mark_bad(&mut self, block: u32) -> Result<(), Error>;
    fn is_free(&mut self, page: u32) -> Result<bool, Error>;
    fn erase(&mut self, block: u32) -> Result<(), Error>;
    fn program(&mut self, page: u32, data: &[u8]) -> Result<(), Error>;
    fn read(&mut self, page: u32, offset: usize, data: &mut [u8]) -> Result<(), Error>;
}
struct Context {
    device: Box<dyn Nand>,
    fault: bool,
    metadata: ReadCache,
}
impl Context {
    fn result(&mut self, result: Result<(), Error>) -> i32 {
        match result {
            Ok(()) => 0,
            Err(error) => {
                self.metadata.clear();
                match error {
                    Error::BadBlock => 1,
                    Error::Uncorrectable => 2,
                    Error::Io => {
                        self.fault = true;
                        3
                    }
                }
            }
        }
    }
}
unsafe fn context<'a>(p: *mut c_void) -> &'a mut Context {
    // SAFETY: the bridge retains a Box<Context>, all access is through &mut Ftl.
    unsafe { &mut *p.cast::<Context>() }
}
#[no_mangle]
unsafe extern "C" fn mosaico_nand_read(
    p: *mut c_void,
    page: u32,
    off: usize,
    len: usize,
    buf: *mut u8,
) -> i32 {
    let c = unsafe { context(p) };
    if c.fault {
        return 3;
    }
    if off.checked_add(len).is_none_or(|end| end > PAGE_SIZE) {
        return c.result(Err(Error::Io));
    }
    let data = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    if c.metadata.read(page, off, data) {
        return 0;
    }
    let r = c.device.read(page, off, data);
    if r.is_ok() {
        c.metadata.insert(page, off, data);
    }
    c.result(r)
}
#[no_mangle]
unsafe extern "C" fn mosaico_nand_prog(p: *mut c_void, page: u32, buf: *const u8) -> i32 {
    let c = unsafe { context(p) };
    if c.fault {
        return 3;
    }
    c.metadata.invalidate_page(page);
    let r = c
        .device
        .program(page, unsafe { core::slice::from_raw_parts(buf, PAGE_SIZE) });
    c.result(r)
}
#[no_mangle]
unsafe extern "C" fn mosaico_nand_erase(p: *mut c_void, block: u32) -> i32 {
    let c = unsafe { context(p) };
    if c.fault {
        return 3;
    }
    c.metadata.invalidate_block(block);
    let r = c.device.erase(block);
    c.result(r)
}
#[no_mangle]
unsafe extern "C" fn mosaico_nand_is_bad(p: *mut c_void, block: u32) -> i32 {
    let c = unsafe { context(p) };
    if c.fault {
        return 1;
    }
    match c.device.is_bad(block) {
        Ok(bad) => bad as i32,
        Err(_) => {
            c.metadata.clear();
            c.fault = true;
            1
        }
    }
}
#[no_mangle]
unsafe extern "C" fn mosaico_nand_mark_bad(p: *mut c_void, block: u32) {
    let c = unsafe { context(p) };
    c.metadata.invalidate_block(block);
    if !c.fault && c.device.mark_bad(block).is_err() {
        c.metadata.clear();
        c.fault = true;
    }
}
#[no_mangle]
unsafe extern "C" fn mosaico_nand_is_free(p: *mut c_void, page: u32) -> i32 {
    let c = unsafe { context(p) };
    if c.fault {
        return 0;
    }
    match c.device.is_free(page) {
        Ok(free) => free as i32,
        Err(_) => {
            c.metadata.clear();
            c.fault = true;
            0
        }
    }
}
extern "C" {
    fn mfs_nand_create(ctx: *mut c_void, blocks: u32, gc_ratio: u8) -> *mut c_void;
    fn mfs_nand_destroy(p: *mut c_void);
    fn mfs_nand_resume(p: *mut c_void) -> i32;
    fn mfs_nand_capacity(p: *mut c_void) -> u32;
    fn mfs_nand_read(p: *mut c_void, sector: u32, buf: *mut u8) -> i32;
    fn mfs_nand_write(p: *mut c_void, sector: u32, buf: *const u8) -> i32;
    fn mfs_nand_trim(p: *mut c_void, sector: u32) -> i32;
    fn mfs_nand_sync(p: *mut c_void) -> i32;
}

pub struct Ftl {
    raw: NonNull<c_void>,
    context: Box<Context>,
    sectors: u32,
}
// SAFETY: C state, callbacks and chip are exclusively accessed through &mut self.
unsafe impl Send for Ftl {}
impl Ftl {
    fn create(device: Box<dyn Nand>) -> Result<Self, Errno> {
        let blocks = device.blocks();
        if !(16..=1024).contains(&blocks) {
            return Err(Errno::INVAL);
        }
        let mut context = Box::new(Context {
            device,
            fault: false,
            metadata: ReadCache::new(),
        });
        let raw = NonNull::new(unsafe {
            mfs_nand_create((&mut *context as *mut Context).cast(), blocks, GC_RATIO)
        })
        .ok_or(Errno::NOMEM)?;
        let sectors = logical_sectors(blocks);
        if sectors < 4 || sectors > unsafe { mfs_nand_capacity(raw.as_ptr()) } {
            unsafe { mfs_nand_destroy(raw.as_ptr()) };
            return Err(Errno::NOSPC);
        }
        Ok(Self {
            raw,
            context,
            sectors,
        })
    }
    /// Only after the caller has erased every usable physical block and scanned markers.
    pub fn on_erased_media(device: Box<dyn Nand>) -> Result<Self, Errno> {
        Self::create(device)
    }
    /// No implicit format and no empty-map fallback when recovery fails.
    pub fn resume(device: Box<dyn Nand>) -> Result<Self, Errno> {
        let mut f = Self::create(device)?;
        let r = unsafe { mfs_nand_resume(f.raw.as_ptr()) };
        f.check(r)?;
        if f.sectors > unsafe { mfs_nand_capacity(f.raw.as_ptr()) } {
            return Err(Errno::NOSPC);
        }
        Ok(f)
    }
    fn check(&mut self, r: i32) -> Result<(), Errno> {
        if self.context.fault {
            return Err(Errno::IO);
        }
        match r {
            0 => Ok(()),
            -5 | -7 => Err(Errno::NOSPC),
            -2 | -6 | -8 => Err(Errno::CORRUPT),
            _ => Err(Errno::IO),
        }
    }
    pub fn geometry(&self) -> Geometry {
        Geometry {
            read_size: PAGE_SIZE as u32,
            prog_size: PAGE_SIZE as u32,
            block_size: PAGE_SIZE as u32 * 2,
            block_count: self.sectors / 2,
            cache_size: PAGE_SIZE as u32,
            lookahead_size: 64,
            block_cycles: -1,
        }
    }
    fn range(&self, block: u32, off: u32, len: usize) -> Result<u32, Errno> {
        if block >= self.sectors / 2
            || off as usize % PAGE_SIZE != 0
            || len % PAGE_SIZE != 0
            || (off as usize)
                .checked_add(len)
                .is_none_or(|end| end > PAGE_SIZE * 2)
        {
            return Err(Errno::INVAL);
        }
        if self.context.fault {
            return Err(Errno::IO);
        }
        Ok(block * 2 + off / PAGE_SIZE as u32)
    }
}
impl BlockDevice for Ftl {
    fn read(&mut self, block: u32, off: u32, data: &mut [u8]) -> Result<(), Errno> {
        let first = self.range(block, off, data.len())?;
        for (i, page) in data.chunks_mut(PAGE_SIZE).enumerate() {
            let r =
                unsafe { mfs_nand_read(self.raw.as_ptr(), first + i as u32, page.as_mut_ptr()) };
            self.check(r)?;
        }
        Ok(())
    }
    fn prog(&mut self, block: u32, off: u32, data: &[u8]) -> Result<(), Errno> {
        let first = self.range(block, off, data.len())?;
        for (i, page) in data.chunks(PAGE_SIZE).enumerate() {
            let r = unsafe { mfs_nand_write(self.raw.as_ptr(), first + i as u32, page.as_ptr()) };
            self.check(r)?;
        }
        Ok(())
    }
    fn erase(&mut self, block: u32) -> Result<(), Errno> {
        let first = self.range(block, 0, PAGE_SIZE * 2)?;
        for page in first..first + 2 {
            let r = unsafe { mfs_nand_trim(self.raw.as_ptr(), page) };
            self.check(r)?;
        }
        Ok(())
    }
    fn sync(&mut self) -> Result<(), Errno> {
        if self.context.fault {
            return Err(Errno::IO);
        }
        let r = unsafe { mfs_nand_sync(self.raw.as_ptr()) };
        self.check(r)
    }
}
impl Drop for Ftl {
    fn drop(&mut self) {
        unsafe { mfs_nand_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(test)]
mod tests;
