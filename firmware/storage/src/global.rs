//! The firmware's one [`Vfs`] and the C entry points over it.
//!
//! Each call finishes synchronously. Same-thread re-entry returns `EBUSY`;
//! a configured runtime may wait for a different RTOS thread without masking
//! interrupts. Interrupt callers can refuse to wait. All paths retain exclusive
//! access to the VFS and its block devices.

use core::cell::UnsafeCell;
use core::ffi::{c_char, c_int, CStr};
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use crate::lfs::{Kind, Metadata, OpenFlags, Whence};
use crate::vfs::Vfs;
use crate::Errno;

struct Global {
    lock: OwnerLock,
    vfs: UnsafeCell<Option<Vfs>>,
}

// SAFETY: access goes through `with`, which hands out one `&mut` at a time.
unsafe impl Sync for Global {}

static GLOBAL: Global = Global {
    lock: OwnerLock::new(),
    vfs: UnsafeCell::new(None),
};

/// Runtime policy for synchronous VFS contention. `context_id` returns a nonzero
/// execution-context identity (an RTOS thread, not a CPU). `wait` yields with interrupts
/// enabled, or returns false if the current context cannot wait (e.g. an ISR).
pub struct LockHooks {
    pub context_id: fn() -> usize,
    pub wait: fn() -> bool,
}

static DEFAULT_HOOKS: LockHooks = LockHooks {
    context_id: || 1,
    wait: || false,
};
static LOCK_HOOKS: AtomicPtr<LockHooks> = AtomicPtr::new(core::ptr::null_mut());

/// Install before starting any concurrent VFS callers. Host users default to
/// immediate `EBUSY`, preserving the same-thread re-entry contract.
pub fn set_lock_hooks(hooks: &'static LockHooks) {
    LOCK_HOOKS.store(core::ptr::from_ref(hooks).cast_mut(), Ordering::Release);
}

struct OwnerLock(AtomicUsize);
impl OwnerLock {
    const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    fn acquire(&self, hooks: &LockHooks) -> Result<OwnerGuard<'_>, Errno> {
        let id = (hooks.context_id)();
        assert_ne!(id, 0, "VFS context identity must be nonzero");
        loop {
            match self
                .0
                .compare_exchange(0, id, Ordering::Acquire, Ordering::Relaxed)
            {
                Ok(_) => return Ok(OwnerGuard(self)),
                Err(owner) if owner == id => return Err(Errno::BUSY),
                Err(_) if !(hooks.wait)() => return Err(Errno::BUSY),
                Err(_) => {}
            }
        }
    }
}

struct OwnerGuard<'a>(&'a OwnerLock);
impl Drop for OwnerGuard<'_> {
    fn drop(&mut self) {
        self.0 .0.store(0, Ordering::Release);
    }
}

/// Install the VFS (once, at boot). Replaces any earlier one.
pub fn install(vfs: Vfs) {
    let _ = with_slot(|slot| *slot = Some(vfs));
}

fn with_slot<R>(f: impl FnOnce(&mut Option<Vfs>) -> R) -> Result<R, Errno> {
    let hooks = LOCK_HOOKS.load(Ordering::Acquire);
    // SAFETY: installed hooks have static lifetime; null selects the default.
    let hooks = unsafe { hooks.as_ref() }.unwrap_or(&DEFAULT_HOOKS);
    let _guard = GLOBAL.lock.acquire(hooks)?;
    // SAFETY: the flag above makes this the only live reference.
    let r = f(unsafe { &mut *GLOBAL.vfs.get() });
    Ok(r)
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn reentry_and_interrupt_contention_return_busy() {
        let lock = OwnerLock::new();
        let core1 = LockHooks {
            context_id: || 1,
            wait: || panic!("same-thread reentry must not wait"),
        };
        let interrupt2 = LockHooks {
            context_id: || 2,
            wait: || false,
        };
        let guard = lock.acquire(&core1).unwrap();
        assert!(matches!(lock.acquire(&core1), Err(Errno::BUSY)));
        assert!(matches!(lock.acquire(&interrupt2), Err(Errno::BUSY)));
        drop(guard);
        assert!(lock.acquire(&interrupt2).is_ok());
    }

    #[test]
    fn other_thread_waits_and_observes_the_released_owner() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        static WAITED: AtomicUsize = AtomicUsize::new(0);
        WAITED.store(0, Ordering::Relaxed);
        let lock = OwnerLock::new();
        let value = AtomicUsize::new(0);
        let core1 = LockHooks {
            context_id: || 1,
            wait: || false,
        };
        let core2 = LockHooks {
            context_id: || 2,
            wait: || {
                WAITED.fetch_add(1, Ordering::Relaxed);
                thread::yield_now();
                true
            },
        };
        thread::scope(|scope| {
            let guard = lock.acquire(&core1).unwrap();
            let reader = scope.spawn(|| {
                let _guard = lock.acquire(&core2).unwrap();
                assert_eq!(value.load(Ordering::Relaxed), 42);
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            while WAITED.load(Ordering::Relaxed) == 0 {
                assert!(Instant::now() < deadline, "second thread did not contend");
                thread::yield_now();
            }
            value.store(42, Ordering::Relaxed);
            drop(guard);
            reader.join().unwrap();
        });
    }
}

/// Run `f` on the installed VFS. `ENOENT` when none is installed (no file
/// system: the same answer as a missing file), `EBUSY` on re-entry.
pub fn with<R>(f: impl FnOnce(&mut Vfs) -> Result<R, Errno>) -> Result<R, Errno> {
    with_slot(|slot| match slot {
        Some(v) => f(v),
        None => Err(Errno::NOENT),
    })?
}

// ---- C entry points ----------------------------------------------------------
//
// Results: >= 0 on success, -errno on failure. Paths are NUL-terminated.

/// Open flags as `mosaico_fs_posix.c` passes them.
pub mod flags {
    pub const READ: i32 = 1;
    pub const WRITE: i32 = 2;
    pub const CREATE: i32 = 4;
    pub const TRUNCATE: i32 = 8;
    pub const APPEND: i32 = 16;
    pub const EXCLUSIVE: i32 = 32;
}

unsafe fn path<'a>(p: *const c_char) -> Result<&'a str, Errno> {
    if p.is_null() {
        return Err(Errno::INVAL);
    }
    // SAFETY: caller passes a NUL-terminated string.
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|_| Errno::INVAL)
}

fn code(r: Result<i32, Errno>) -> c_int {
    match r {
        Ok(v) => v,
        Err(e) => -e.0,
    }
}

fn kind_code(m: Metadata) -> c_int {
    match m.kind {
        Kind::File => 1,
        Kind::Dir => 2,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_open(p: *const c_char, f: c_int) -> c_int {
    let flags = OpenFlags {
        read: f & flags::READ != 0,
        write: f & flags::WRITE != 0,
        create: f & flags::CREATE != 0,
        truncate: f & flags::TRUNCATE != 0,
        append: f & flags::APPEND != 0,
        exclusive: f & flags::EXCLUSIVE != 0,
    };
    code(unsafe { path(p) }.and_then(|p| with(|v| v.open(p, flags))))
}

#[no_mangle]
pub extern "C" fn mosaico_vfs_close(fd: c_int) -> c_int {
    code(with(|v| v.close(fd)).map(|()| 0))
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_read(fd: c_int, buf: *mut u8, n: usize) -> c_int {
    if buf.is_null() && n > 0 {
        return -Errno::INVAL.0;
    }
    // SAFETY: caller passes a writable buffer of n bytes.
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, n) };
    code(with(|v| v.read(fd, buf)).map(|n| n as i32))
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_write(fd: c_int, buf: *const u8, n: usize) -> c_int {
    if buf.is_null() && n > 0 {
        return -Errno::INVAL.0;
    }
    // SAFETY: caller passes a readable buffer of n bytes.
    let data = unsafe { core::slice::from_raw_parts(buf, n) };
    code(with(|v| v.write(fd, data)).map(|n| n as i32))
}

/// whence: 0 set, 1 cur, 2 end. Returns the new position.
#[no_mangle]
pub extern "C" fn mosaico_vfs_lseek(fd: c_int, off: i64, whence: c_int) -> i64 {
    let w = match whence {
        0 => Whence::Set,
        1 => Whence::Cur,
        2 => Whence::End,
        _ => return -(Errno::INVAL.0 as i64),
    };
    match with(|v| v.seek(fd, off, w)) {
        Ok(p) => p as i64,
        Err(e) => -(e.0 as i64),
    }
}

/// Kind (1 file, 2 dir) and size of an open file.
#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_fstat(fd: c_int, kind: *mut c_int, size: *mut u32) -> c_int {
    code(with(|v| v.fstat(fd)).map(|m| {
        // SAFETY: caller passes writable outputs.
        unsafe {
            kind.write(kind_code(m));
            size.write(m.size);
        }
        0
    }))
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_stat(
    p: *const c_char,
    kind: *mut c_int,
    size: *mut u32,
) -> c_int {
    code(
        unsafe { path(p) }
            .and_then(|p| with(|v| v.stat(p)))
            .map(|m| {
                // SAFETY: caller passes writable outputs.
                unsafe {
                    kind.write(kind_code(m));
                    size.write(m.size);
                }
                0
            }),
    )
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_mkdir(p: *const c_char) -> c_int {
    code(
        unsafe { path(p) }
            .and_then(|p| with(|v| v.mkdir(p)))
            .map(|()| 0),
    )
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_rmdir(p: *const c_char) -> c_int {
    code(
        unsafe { path(p) }
            .and_then(|p| with(|v| v.rmdir(p)))
            .map(|()| 0),
    )
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_unlink(p: *const c_char) -> c_int {
    code(
        unsafe { path(p) }
            .and_then(|p| with(|v| v.unlink(p)))
            .map(|()| 0),
    )
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_rename(a: *const c_char, b: *const c_char) -> c_int {
    let r = unsafe { path(a) }.and_then(|a| unsafe { path(b) }.map(|b| (a, b)));
    code(r.and_then(|(a, b)| with(|v| v.rename(a, b))).map(|()| 0))
}

#[no_mangle]
pub extern "C" fn mosaico_vfs_fsync(fd: c_int) -> c_int {
    code(with(|v| v.fsync(fd)).map(|()| 0))
}

#[no_mangle]
pub extern "C" fn mosaico_vfs_ftruncate(fd: c_int, size: u32) -> c_int {
    code(with(|v| v.ftruncate(fd, size)).map(|()| 0))
}

/// A directory handle (> 0).
#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_opendir(p: *const c_char) -> c_int {
    code(unsafe { path(p) }.and_then(|p| with(|v| v.opendir(p))))
}

/// 1 with an entry (`name` gets up to 256 bytes, NUL-terminated), 0 at the end.
#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_readdir(
    h: c_int,
    name: *mut c_char,
    kind: *mut c_int,
) -> c_int {
    code(with(|v| {
        Ok(match v.readdir(h)? {
            None => 0,
            Some(e) => {
                let bytes = e.name.as_bytes();
                let n = bytes.len().min(255);
                // SAFETY: caller passes 256 writable bytes and an int.
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), name.cast::<u8>(), n);
                    name.add(n).write(0);
                    kind.write(kind_code(e.meta));
                }
                1
            }
        })
    }))
}

#[no_mangle]
pub extern "C" fn mosaico_vfs_closedir(h: c_int) -> c_int {
    code(with(|v| v.closedir(h)).map(|()| 0))
}

#[no_mangle]
pub unsafe extern "C" fn mosaico_vfs_statvfs(
    p: *const c_char,
    block_size: *mut u32,
    blocks: *mut u32,
    free: *mut u32,
) -> c_int {
    code(
        unsafe { path(p) }
            .and_then(|p| with(|v| v.space(p)))
            .map(|s| {
                // SAFETY: caller passes writable outputs.
                unsafe {
                    block_size.write(s.block_size);
                    blocks.write(s.blocks);
                    free.write(s.free);
                }
                0
            }),
    )
}
