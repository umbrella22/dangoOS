//! One littlefs file system on a block device.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_void, CStr};
use core::ptr::NonNull;

use crate::Errno;

/// The storage under a file system. Offsets are inside `block`.
pub trait BlockDevice {
    fn read(&mut self, block: u32, off: u32, buf: &mut [u8]) -> Result<(), Errno>;
    fn prog(&mut self, block: u32, off: u32, data: &[u8]) -> Result<(), Errno>;
    fn erase(&mut self, block: u32) -> Result<(), Errno>;
    fn sync(&mut self) -> Result<(), Errno> {
        Ok(())
    }
}

/// littlefs configuration (see `struct lfs_config`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub read_size: u32,
    pub prog_size: u32,
    pub block_size: u32,
    pub block_count: u32,
    pub cache_size: u32,
    pub lookahead_size: u32,
    pub block_cycles: i32,
}

impl Geometry {
    /// 4 KiB flash sectors, as ESP-IDF's esp_littlefs formats them.
    pub const fn flash(block_count: u32) -> Self {
        Self {
            read_size: 64,
            prog_size: 64,
            block_size: 4096,
            block_count,
            cache_size: 512,
            lookahead_size: 64,
            block_cycles: 512,
        }
    }
}

/// What a path names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub kind: Kind,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub meta: Metadata,
}

/// How to open a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub truncate: bool,
    pub append: bool,
    pub exclusive: bool,
}

impl OpenFlags {
    pub const READ: Self = Self {
        read: true,
        write: false,
        create: false,
        truncate: false,
        append: false,
        exclusive: false,
    };
    /// Create or replace.
    pub const WRITE: Self = Self {
        read: false,
        write: true,
        create: true,
        truncate: true,
        append: false,
        exclusive: false,
    };

    fn lfs(self) -> c_int {
        let mut f = match (self.read, self.write) {
            (_, false) => 1,    // LFS_O_RDONLY
            (false, true) => 2, // LFS_O_WRONLY
            (true, true) => 3,  // LFS_O_RDWR
        };
        if self.create {
            f |= 0x0100;
        }
        if self.exclusive {
            f |= 0x0200;
        }
        if self.truncate {
            f |= 0x0400;
        }
        if self.append {
            f |= 0x0800;
        }
        f
    }

    pub fn writes(self) -> bool {
        self.write || self.create || self.truncate || self.append
    }
}

/// `whence` for [`Fs::seek`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Whence {
    Set,
    Cur,
    End,
}

#[allow(improper_ctypes)]
extern "C" {
    fn mfs_create(ctx: *mut c_void, g: *const Geometry) -> *mut c_void;
    fn mfs_destroy(m: *mut c_void);
    fn mfs_mount(m: *mut c_void) -> c_int;
    fn mfs_unmount(m: *mut c_void) -> c_int;
    fn mfs_format(m: *mut c_void) -> c_int;
    fn mfs_mkdir(m: *mut c_void, p: *const c_char) -> c_int;
    fn mfs_remove(m: *mut c_void, p: *const c_char) -> c_int;
    fn mfs_rename(m: *mut c_void, a: *const c_char, b: *const c_char) -> c_int;
    fn mfs_make_consistent(m: *mut c_void) -> c_int;
    fn mfs_fs_size(m: *mut c_void) -> i32;
    fn mfs_stat(m: *mut c_void, p: *const c_char, kind: *mut c_int, size: *mut u32) -> c_int;
    fn mfs_file_open(
        m: *mut c_void,
        out: *mut *mut c_void,
        p: *const c_char,
        flags: c_int,
    ) -> c_int;
    fn mfs_file_close(m: *mut c_void, f: *mut c_void) -> c_int;
    fn mfs_file_read(m: *mut c_void, f: *mut c_void, buf: *mut c_void, n: u32) -> i32;
    fn mfs_file_write(m: *mut c_void, f: *mut c_void, buf: *const c_void, n: u32) -> i32;
    fn mfs_file_seek(m: *mut c_void, f: *mut c_void, off: i32, whence: c_int) -> i32;
    fn mfs_file_size(m: *mut c_void, f: *mut c_void) -> i32;
    fn mfs_file_sync(m: *mut c_void, f: *mut c_void) -> c_int;
    fn mfs_file_truncate(m: *mut c_void, f: *mut c_void, size: u32) -> c_int;
    fn mfs_dir_open(m: *mut c_void, out: *mut *mut c_void, p: *const c_char) -> c_int;
    fn mfs_dir_read(
        m: *mut c_void,
        d: *mut c_void,
        kind: *mut c_int,
        size: *mut u32,
        name: *mut c_char,
    ) -> c_int;
    fn mfs_dir_close(m: *mut c_void, d: *mut c_void) -> c_int;
}

type Device = Box<dyn BlockDevice + Send>;

// ---- block device callbacks (ctx = *mut Device) -----------------------------

fn bd_result(r: Result<(), Errno>) -> c_int {
    match r {
        Ok(()) => 0,
        Err(e) => -e.0,
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_fs_bd_read(
    ctx: *mut c_void,
    block: u32,
    off: u32,
    buf: *mut u8,
    size: u32,
) -> c_int {
    // SAFETY: ctx is the `Device` box owned by the `Fs` that littlefs is
    // working for; buf/size describe littlefs' own buffer.
    let (dev, buf) = unsafe {
        (
            &mut *ctx.cast::<Device>(),
            core::slice::from_raw_parts_mut(buf, size as usize),
        )
    };
    bd_result(dev.read(block, off, buf))
}

#[no_mangle]
unsafe extern "C" fn mosaico_fs_bd_prog(
    ctx: *mut c_void,
    block: u32,
    off: u32,
    buf: *const u8,
    size: u32,
) -> c_int {
    // SAFETY: as in `mosaico_fs_bd_read`.
    let (dev, data) = unsafe {
        (
            &mut *ctx.cast::<Device>(),
            core::slice::from_raw_parts(buf, size as usize),
        )
    };
    bd_result(dev.prog(block, off, data))
}

#[no_mangle]
unsafe extern "C" fn mosaico_fs_bd_erase(ctx: *mut c_void, block: u32) -> c_int {
    // SAFETY: as in `mosaico_fs_bd_read`.
    bd_result(unsafe { &mut *ctx.cast::<Device>() }.erase(block))
}

#[no_mangle]
unsafe extern "C" fn mosaico_fs_bd_sync(ctx: *mut c_void) -> c_int {
    // SAFETY: as in `mosaico_fs_bd_read`.
    bd_result(unsafe { &mut *ctx.cast::<Device>() }.sync())
}

/// littlefs error codes are negated Linux errnos; the firmware's C library
/// (newlib) numbers a few differently.
fn lfs_errno(code: i32) -> Errno {
    match -code {
        39 => Errno::NOTEMPTY,
        36 => Errno::NAMETOOLONG,
        84 => Errno::CORRUPT,
        61 => Errno::NOENT, // LFS_ERR_NOATTR
        e => Errno(e),
    }
}

fn check(code: c_int) -> Result<(), Errno> {
    if code < 0 {
        Err(lfs_errno(code))
    } else {
        Ok(())
    }
}

fn check_len(code: i32) -> Result<u32, Errno> {
    if code < 0 {
        Err(lfs_errno(code))
    } else {
        Ok(code as u32)
    }
}

/// A path as a C string. littlefs paths are relative to the file system root.
fn c_path(path: &str) -> Result<Vec<u8>, Errno> {
    if path.as_bytes().contains(&0) {
        return Err(Errno::INVAL);
    }
    let mut v = Vec::with_capacity(path.len() + 1);
    v.extend_from_slice(path.as_bytes());
    v.push(0);
    Ok(v)
}

/// An open littlefs file (owned by an [`Fs`]; close it there).
pub struct File(NonNull<c_void>);

// SAFETY: a `File` is only used through its `Fs`, which serialises access.
unsafe impl Send for File {}

/// A mounted littlefs.
pub struct Fs {
    raw: NonNull<c_void>,
    // Boxed twice so the pointer littlefs keeps (to the inner fat box)
    // stays put when the `Fs` moves.
    _dev: Box<Device>,
    read_only: bool,
    geometry: Geometry,
}

// SAFETY: the C state is owned and only reached through `&mut Fs`.
unsafe impl Send for Fs {}

impl Fs {
    fn create(dev: Device, geometry: Geometry) -> Result<(NonNull<c_void>, Box<Device>), Errno> {
        let mut dev = Box::new(dev);
        let ctx: *mut Device = &mut *dev;
        // SAFETY: geometry is read during the call; ctx outlives the C state
        // (both are owned by the returned `Fs`).
        let raw = unsafe { mfs_create(ctx.cast(), &geometry) };
        let raw = NonNull::new(raw).ok_or(Errno::NOMEM)?;
        Ok((raw, dev))
    }

    /// Mount an existing file system.
    pub fn mount(dev: Device, geometry: Geometry, read_only: bool) -> Result<Self, Errno> {
        let (raw, dev) = Self::create(dev, geometry)?;
        // SAFETY: raw came from mfs_create.
        if let Err(e) = check(unsafe { mfs_mount(raw.as_ptr()) }) {
            unsafe { mfs_destroy(raw.as_ptr()) };
            return Err(e);
        }
        Ok(Self {
            raw,
            _dev: dev,
            read_only,
            geometry,
        })
    }

    /// Mount, formatting first when there is no valid file system.
    pub fn mount_or_format(dev: Device, geometry: Geometry) -> Result<(Self, bool), Errno> {
        let (raw, dev) = Self::create(dev, geometry)?;
        let m = raw.as_ptr();
        // SAFETY: raw came from mfs_create.
        let mut formatted = false;
        let mut r = check(unsafe { mfs_mount(m) });
        if r.is_err() {
            formatted = true;
            r = check(unsafe { mfs_format(m) }).and_then(|()| check(unsafe { mfs_mount(m) }));
        }
        if let Err(e) = r {
            unsafe { mfs_destroy(m) };
            return Err(e);
        }
        Ok((
            Self {
                raw,
                _dev: dev,
                read_only: false,
                geometry,
            },
            formatted,
        ))
    }

    /// Explicitly replace the filesystem. Ordinary mount never calls this.
    pub fn format(dev: Device, geometry: Geometry) -> Result<Self, Errno> {
        let (raw, dev) = Self::create(dev, geometry)?;
        let r = check(unsafe { mfs_format(raw.as_ptr()) })
            .and_then(|()| check(unsafe { mfs_mount(raw.as_ptr()) }));
        if let Err(e) = r {
            unsafe { mfs_destroy(raw.as_ptr()) };
            return Err(e);
        }
        Ok(Self { raw, _dev: dev, read_only: false, geometry })
    }

    fn m(&self) -> *mut c_void {
        self.raw.as_ptr()
    }

    fn writable(&self) -> Result<(), Errno> {
        if self.read_only {
            Err(Errno::ROFS)
        } else {
            Ok(())
        }
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Blocks in use.
    pub fn used_blocks(&mut self) -> Result<u32, Errno> {
        // SAFETY: mounted state.
        check_len(unsafe { mfs_fs_size(self.m()) })
    }

    pub fn stat(&mut self, path: &str) -> Result<Metadata, Errno> {
        let p = c_path(path)?;
        let (mut kind, mut size) = (0, 0);
        // SAFETY: p is NUL-terminated; outputs are locals.
        check(unsafe { mfs_stat(self.m(), p.as_ptr().cast(), &mut kind, &mut size) })?;
        Ok(Metadata {
            kind: if kind == 2 { Kind::Dir } else { Kind::File },
            size,
        })
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), Errno> {
        self.writable()?;
        let p = c_path(path)?;
        // SAFETY: p is NUL-terminated.
        check(unsafe { mfs_mkdir(self.m(), p.as_ptr().cast()) })
    }

    /// Remove a file or an empty directory.
    pub fn remove(&mut self, path: &str) -> Result<(), Errno> {
        self.writable()?;
        let p = c_path(path)?;
        // SAFETY: p is NUL-terminated.
        check(unsafe { mfs_remove(self.m(), p.as_ptr().cast()) })
    }

    /// Rename, replacing a file or an empty directory at `to`.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Errno> {
        self.writable()?;
        let (a, b) = (c_path(from)?, c_path(to)?);
        // SAFETY: both NUL-terminated.
        check(unsafe { mfs_rename(self.m(), a.as_ptr().cast(), b.as_ptr().cast()) })
    }

    /// Complete pending LittleFS metadata moves before inspecting a recovery journal.
    pub fn make_consistent(&mut self) -> Result<(), Errno> {
        self.writable()?;
        // SAFETY: exclusively borrowed, mounted LittleFS state.
        check(unsafe { mfs_make_consistent(self.m()) })
    }

    pub fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, Errno> {
        let p = c_path(path)?;
        let mut d = core::ptr::null_mut();
        // SAFETY: p is NUL-terminated; d receives a handle closed below.
        check(unsafe { mfs_dir_open(self.m(), &mut d, p.as_ptr().cast()) })?;
        let mut out = Vec::new();
        let mut name = [0u8; 256];
        let result = loop {
            let (mut kind, mut size) = (0, 0);
            // SAFETY: d is open; name holds LFS_NAME_MAX + 1 bytes.
            let r = unsafe {
                mfs_dir_read(self.m(), d, &mut kind, &mut size, name.as_mut_ptr().cast())
            };
            if r < 0 {
                break Err(Errno(-r));
            }
            if r == 0 {
                break Ok(());
            }
            let n = CStr::from_bytes_until_nul(&name)
                .map(|c| String::from_utf8_lossy(c.to_bytes()).into_owned())
                .unwrap_or_default();
            if n == "." || n == ".." {
                continue;
            }
            out.push(DirEntry {
                name: n,
                meta: Metadata {
                    kind: if kind == 2 { Kind::Dir } else { Kind::File },
                    size,
                },
            });
        };
        // SAFETY: d is open.
        unsafe { mfs_dir_close(self.m(), d) };
        result.map(|()| out)
    }

    pub fn open(&mut self, path: &str, flags: OpenFlags) -> Result<File, Errno> {
        if flags.writes() {
            self.writable()?;
        }
        let p = c_path(path)?;
        let mut f = core::ptr::null_mut();
        // SAFETY: p is NUL-terminated; f receives a handle owned by `File`.
        check(unsafe { mfs_file_open(self.m(), &mut f, p.as_ptr().cast(), flags.lfs()) })?;
        NonNull::new(f).map(File).ok_or(Errno::NOMEM)
    }

    pub fn close(&mut self, file: File) -> Result<(), Errno> {
        // SAFETY: file is open on this Fs; it is consumed.
        check(unsafe { mfs_file_close(self.m(), file.0.as_ptr()) })
    }

    pub fn read(&mut self, file: &mut File, buf: &mut [u8]) -> Result<usize, Errno> {
        // SAFETY: file is open on this Fs; buf is writable for its length.
        check_len(unsafe {
            mfs_file_read(
                self.m(),
                file.0.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
            )
        })
        .map(|n| n as usize)
    }

    pub fn write(&mut self, file: &mut File, data: &[u8]) -> Result<usize, Errno> {
        // SAFETY: file is open on this Fs.
        check_len(unsafe {
            mfs_file_write(
                self.m(),
                file.0.as_ptr(),
                data.as_ptr().cast(),
                data.len() as u32,
            )
        })
        .map(|n| n as usize)
    }

    pub fn seek(&mut self, file: &mut File, off: i64, whence: Whence) -> Result<u32, Errno> {
        let off = i32::try_from(off).map_err(|_| Errno::INVAL)?;
        let w = match whence {
            Whence::Set => 0,
            Whence::Cur => 1,
            Whence::End => 2,
        };
        // SAFETY: file is open on this Fs.
        check_len(unsafe { mfs_file_seek(self.m(), file.0.as_ptr(), off, w) })
    }

    pub fn size(&mut self, file: &mut File) -> Result<u32, Errno> {
        // SAFETY: file is open on this Fs.
        check_len(unsafe { mfs_file_size(self.m(), file.0.as_ptr()) })
    }

    pub fn sync(&mut self, file: &mut File) -> Result<(), Errno> {
        // SAFETY: file is open on this Fs.
        check(unsafe { mfs_file_sync(self.m(), file.0.as_ptr()) })
    }

    pub fn truncate(&mut self, file: &mut File, size: u32) -> Result<(), Errno> {
        self.writable()?;
        // SAFETY: file is open on this Fs.
        check(unsafe { mfs_file_truncate(self.m(), file.0.as_ptr(), size) })
    }
}

impl Drop for Fs {
    fn drop(&mut self) {
        // SAFETY: mounted state; nothing uses it afterwards. Open files must
        // have been closed by the owner (the VFS closes them first).
        unsafe {
            mfs_unmount(self.m());
            mfs_destroy(self.m());
        }
    }
}

/// A block device in RAM (tests, and images built on the host).
pub struct RamDevice {
    pub data: Vec<u8>,
    pub block_size: u32,
}

impl RamDevice {
    pub fn new(block_size: u32, block_count: u32) -> Self {
        Self {
            data: alloc::vec![0xff; (block_size * block_count) as usize],
            block_size,
        }
    }

    pub fn from_image(data: Vec<u8>, block_size: u32) -> Self {
        Self { data, block_size }
    }

    fn range(&self, block: u32, off: u32, len: usize) -> Result<core::ops::Range<usize>, Errno> {
        let start = block as usize * self.block_size as usize + off as usize;
        let end = start + len;
        if end > self.data.len() {
            return Err(Errno::IO);
        }
        Ok(start..end)
    }
}

impl BlockDevice for RamDevice {
    fn read(&mut self, block: u32, off: u32, buf: &mut [u8]) -> Result<(), Errno> {
        let r = self.range(block, off, buf.len())?;
        buf.copy_from_slice(&self.data[r]);
        Ok(())
    }
    fn prog(&mut self, block: u32, off: u32, data: &[u8]) -> Result<(), Errno> {
        let r = self.range(block, off, data.len())?;
        // NOR flash: programming only clears bits.
        for (d, s) in self.data[r].iter_mut().zip(data) {
            *d &= *s;
        }
        Ok(())
    }
    fn erase(&mut self, block: u32) -> Result<(), Errno> {
        let r = self.range(block, 0, self.block_size as usize)?;
        self.data[r].fill(0xff);
        Ok(())
    }
}
