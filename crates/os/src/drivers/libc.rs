//! The C library services the Lua core needs from the firmware.
//!
//! `lua-engine` links newlib's `libc.a`/`libm.a` from the riscv32-esp-elf
//! toolchain for `snprintf`, `strtod` and the math functions. newlib expects
//! an operating system underneath; this module is that layer:
//!
//! - newlib's `_malloc_r` family (used by newlib itself, e.g. `strtod` on
//!   long numbers), forwarded to esp-alloc's C `malloc`.
//! - The process syscalls (`_sbrk_r`, `_kill_r`, ...), which fail with
//!   `ENOSYS`. File syscalls come from `fs`; this module only adds
//!   `mosaico_console_write` for stdout/stderr.
//! - `abort` / `_exit`, which Lua only reaches on an internal panic.
//!
//! Lua's own heap does not come through here: it uses `lua_Alloc` on PSRAM
//! (see `tasks::lua`).

use core::ffi::{c_char, c_int, c_void};

const ENOSYS: c_int = 88;
const ENOMEM: c_int = 12;

// esp-alloc's `compat` feature (on by default) defines these on the Rust
// heap; newlib's reentrant variants below forward to them.
extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn free(p: *mut c_void);
    fn calloc(n: u32, size: usize) -> *mut c_void;
    fn realloc(p: *mut c_void, size: usize) -> *mut c_void;
}

// newlib's reentrant entry points take a `struct _reent *` first.

#[no_mangle]
pub unsafe extern "C" fn _malloc_r(_r: *mut c_void, size: usize) -> *mut c_void {
    unsafe { malloc(size) }
}
#[no_mangle]
pub unsafe extern "C" fn _free_r(_r: *mut c_void, p: *mut c_void) {
    unsafe { free(p) }
}
#[no_mangle]
pub unsafe extern "C" fn _calloc_r(_r: *mut c_void, n: usize, size: usize) -> *mut c_void {
    unsafe { calloc(n as u32, size) }
}
#[no_mangle]
pub unsafe extern "C" fn _realloc_r(_r: *mut c_void, p: *mut c_void, size: usize) -> *mut c_void {
    unsafe { realloc(p, size) }
}

// ---- syscalls: no files, no processes -------------------------------------

/// `errno` lives in the reent struct; newlib's `__errno()` finds it. The
/// stubs only return -1 and set it through the pointer they were given.
unsafe fn fail(r: *mut c_void, errno: c_int) -> c_int {
    if !r.is_null() {
        // `struct _reent` starts with `int _errno`.
        unsafe { r.cast::<c_int>().write(errno) };
    }
    -1
}

/// stdout/stderr from newlib (Lua's `print`, `io.write`), line-buffered into
/// the log. File descriptors and the other file syscalls live in
/// `fs` (`mosaico_fs_posix.c`), which routes fds 1/2 here.
#[no_mangle]
pub unsafe extern "C" fn mosaico_console_write(buf: *const c_char, n: usize) {
    // SAFETY: the C side passes a valid buffer of n bytes.
    let bytes = unsafe { core::slice::from_raw_parts(buf.cast::<u8>(), n) };
    console_line::push(bytes);
}

mod console_line {
    use core::cell::RefCell;
    use critical_section::Mutex;

    const CAP: usize = 160;
    static LINE: Mutex<RefCell<([u8; CAP], usize)>> = Mutex::new(RefCell::new(([0; CAP], 0)));

    fn flush(line: &[u8]) {
        let text = core::str::from_utf8(line).unwrap_or("<non-utf8 output>");
        log::info!("lua> {text}");
    }

    pub fn push(bytes: &[u8]) {
        for &b in bytes {
            let mut out = None;
            critical_section::with(|cs| {
                let mut l = LINE.borrow_ref_mut(cs);
                let (buf, len) = &mut *l;
                if b != b'\n' && b != b'\r' {
                    buf[*len] = b;
                    *len += 1;
                }
                if b == b'\n' || *len == CAP {
                    out = Some((*buf, *len));
                    *len = 0;
                }
            });
            if let Some((buf, len)) = out {
                flush(&buf[..len]);
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn _sbrk_r(r: *mut c_void, _incr: isize) -> *mut c_void {
    unsafe { fail(r, ENOMEM) };
    usize::MAX as *mut c_void // (void *)-1
}
#[no_mangle]
pub unsafe extern "C" fn _kill_r(r: *mut c_void, _pid: c_int, _sig: c_int) -> c_int {
    unsafe { fail(r, ENOSYS) }
}
#[no_mangle]
pub extern "C" fn _getpid_r(_r: *mut c_void) -> c_int {
    1
}

#[no_mangle]
pub extern "C" fn abort() -> ! {
    panic!("C abort() (Lua internal panic)");
}
#[no_mangle]
pub extern "C" fn _exit(code: c_int) -> ! {
    panic!("C _exit({code})");
}

/// Canary for newlib objects built with `-fstack-protector`. esp-hal provides
/// `__stack_chk_fail`; defining the guard here keeps newlib's own
/// `stack_protector.o` (a second `__stack_chk_fail`) out of the link.
#[no_mangle]
pub static mut __stack_chk_guard: u32 = 0xe2de_e396;

extern "C" {
    static mut _impure_ptr: *mut c_void;
}

/// newlib's per-thread state. Only the Lua task calls into newlib, so the
/// single global `_impure_ptr` (newlib's own default) is enough.
#[no_mangle]
pub extern "C" fn __getreent() -> *mut c_void {
    unsafe { _impure_ptr }
}

#[no_mangle]
pub extern "C" fn _fcntl(_fd: c_int, _cmd: c_int, _arg: c_int) -> c_int {
    -1
}
