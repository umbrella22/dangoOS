//! One Lua state running one script as a cooperative task.
//!
//! The script runs on its own coroutine. A count hook yields it every
//! [`Limits::slice_hooks`] x [`Limits::hook_interval`] VM instructions, and
//! host functions can yield it with a [`Request`] (sleep, present a frame).
//! [`Vm::resume`] therefore always returns quickly and the caller (an Embassy
//! task in the firmware) decides when to continue.

use alloc::{boxed::Box, string::String, vec::Vec};
use core::{
    alloc::Layout,
    any::Any,
    cell::Cell,
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

use crate::call::{Call, Error, Function, Ret};
use crate::ffi::*;
use crate::platform::{self, esp_err, Blit, DisplayInfo, Enter, HeapInfo, TouchPoint};

/// Resource limits for one script.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Cap on bytes allocated by Lua (soft while a host function runs).
    pub memory_bytes: usize,
    /// VM instructions between two hook calls.
    pub hook_interval: u32,
    /// Hook calls per time slice before the script is yielded.
    pub slice_hooks: u32,
    /// Hook calls without any yield before a script that cannot be yielded
    /// (inside a C call such as `table.sort` with a Lua comparator) is
    /// stopped with an error.
    pub max_blocked_hooks: u32,
    /// Microsecond clock. When set, slices and the blocked limit are measured
    /// in time ([`Limits::slice_us`], [`Limits::max_blocked_us`]) instead of
    /// hook counts: a few VM instructions can call expensive C functions
    /// (`string.format` with soft floats), so counts alone make uneven slices.
    pub clock: Option<fn() -> u64>,
    pub slice_us: u32,
    pub max_blocked_us: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 2 * 1024 * 1024,
            hook_interval: 1000,
            slice_hooks: 8,
            max_blocked_hooks: 20_000,
            clock: None,
            slice_us: 4_000,
            max_blocked_us: 2_000_000,
        }
    }
}

/// Memory for Lua's heap. The firmware routes this to PSRAM.
pub struct RawAllocator {
    pub alloc: unsafe fn(Layout) -> *mut u8,
    pub dealloc: unsafe fn(*mut u8, Layout),
}

unsafe fn global_alloc(layout: Layout) -> *mut u8 {
    alloc::alloc::alloc(layout)
}
unsafe fn global_dealloc(ptr: *mut u8, layout: Layout) {
    alloc::alloc::dealloc(ptr, layout)
}

/// The Rust global allocator.
pub static GLOBAL_ALLOCATOR: RawAllocator = RawAllocator {
    alloc: global_alloc,
    dealloc: global_dealloc,
};

/// What the embedding provides to a script. Everything but `print` is
/// optional; the defaults behave like a board without that hardware.
pub trait Host: Any {
    /// A coherent copy from the sensor task; this method must never read a bus.
    fn hardware_snapshot(&mut self) -> crate::HardwareSnapshot {
        crate::HardwareSnapshot::default()
    }
    /// Queue a bounded request; success means accepted, not physically completed.
    fn hardware_request(&mut self, request: crate::HardwareRequest) -> Result<(), i32> {
        let _ = request;
        Err(esp_err::NOT_SUPPORTED)
    }

    /// One line from Lua's `print` (no trailing newline).
    fn print(&mut self, line: &[u8]);

    /// A log line from a C module (`E (display_core) present failed: ...`).
    fn log(&mut self, line: &[u8]) {
        log_line(line);
    }

    /// Wall clock for `os.time`/`system.time`; `None` counts from boot.
    fn unix_time_us(&self) -> Option<i64> {
        None
    }

    /// `display.open()`: take the screen for this script, or an ESP error
    /// code (`esp_err::INVALID_STATE` when another owner holds it).
    fn display_open(&mut self) -> Result<DisplayInfo, i32> {
        Err(esp_err::NOT_SUPPORTED)
    }

    /// Geometry while the screen is open.
    fn display_info(&self) -> Option<DisplayInfo> {
        None
    }

    /// The script's screen was closed (explicitly, by GC or at exit).
    fn display_close(&mut self) {}

    /// Pixels of one dirty rectangle from `display:present()`. The script
    /// is suspended with [`Request::Present`] right after the last
    /// rectangle of a frame, and its framebuffer stays untouched until it is
    /// resumed, so the host may keep reading `blit.pixels` until then.
    fn display_blit(&mut self, blit: &Blit) -> Result<(), i32> {
        let _ = blit;
        Err(esp_err::NOT_SUPPORTED)
    }

    /// Current touch points; the count written to `out`.
    fn touch(&mut self, out: &mut [TouchPoint]) -> Result<usize, i32> {
        let _ = out;
        Err(esp_err::NOT_SUPPORTED)
    }

    /// `system.heap.get_info(caps)`.
    fn heap_info(&mut self, caps: u32) -> HeapInfo {
        let _ = caps;
        HeapInfo::default()
    }
}

/// Why a script stopped running for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `delay.delay_ms(ms)`: resume after `ms` milliseconds.
    Sleep { ms: u32 },
    /// `display.present()`: show the frame, then resume.
    Present,
    /// Embedding-defined request.
    Host(u32),
}

/// Result of one [`Vm::resume`].
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Time slice used up; resume again after letting other tasks run.
    Slice,
    /// A host function asked to wait for something.
    Request(Request),
    /// The script returned.
    Done,
    /// The script was stopped by [`Vm::interrupt`].
    Interrupted,
    /// Runtime or syntax error, with a traceback when available.
    Error(String),
}

/// Allocator state, separate from [`Ctx`] so a host function holding
/// `&mut Ctx` does not alias the allocator's view.
struct Heap {
    allocator: &'static RawAllocator,
    used: Cell<usize>,
    peak: Cell<usize>,
    limit: usize,
    /// Non-zero while a host function runs: the cap is not enforced then,
    /// so a Lua API call made from Rust only fails on real exhaustion.
    in_host: Cell<u32>,
}

pub(crate) struct Ctx {
    pub(crate) data_root: Vec<u8>,
    pub(crate) host: Box<dyn Host>,
    heap: *const Heap,
    pub(crate) limits: Limits,
    thread: *mut lua_State,
    hooks: u32,
    since_yield: u32,
    slice_start: u64,
    last_yield: u64,
    interrupt: bool,
    request: Option<Request>,
    error_text: String,
    error_arg: c_int,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Fresh,
    Loaded,
    Finished,
}

pub struct Vm {
    main: *mut lua_State,
    thread: *mut lua_State,
    ctx: *mut Ctx,
    heap: *mut Heap,
    state: State,
}

const ALIGN: usize = 8;

unsafe extern "C" fn lua_alloc(
    ud: *mut c_void,
    ptr: *mut c_void,
    osize: usize,
    nsize: usize,
) -> *mut c_void {
    let heap = &*(ud as *const Heap);
    // With ptr == NULL, osize encodes the object type, not a size.
    let old = if ptr.is_null() { 0 } else { osize };
    if nsize == 0 {
        if !ptr.is_null() {
            (heap.allocator.dealloc)(ptr.cast(), Layout::from_size_align_unchecked(old, ALIGN));
            heap.used.set(heap.used.get() - old);
        }
        return ptr::null_mut();
    }
    let used = heap.used.get() - old;
    if nsize > old && heap.in_host.get() == 0 && used + nsize > heap.limit {
        return ptr::null_mut(); // Lua collects garbage and retries
    }
    let new = (heap.allocator.alloc)(Layout::from_size_align_unchecked(nsize, ALIGN));
    if new.is_null() {
        // Lua requires shrinking to succeed: keep the old block.
        return if nsize <= old { ptr } else { ptr::null_mut() };
    }
    if !ptr.is_null() {
        ptr::copy_nonoverlapping(ptr.cast::<u8>(), new, old.min(nsize));
        (heap.allocator.dealloc)(ptr.cast(), Layout::from_size_align_unchecked(old, ALIGN));
    }
    heap.used.set(used + nsize);
    if used + nsize > heap.peak.get() {
        heap.peak.set(used + nsize);
    }
    new.cast()
}

impl Vm {
    /// Set this VM's storage root, including code run during `lua_close`.
    /// This is a storage convention, not an `io` sandbox.
    pub fn set_storage_root(&mut self, path: &str) -> Result<(), String> {
        if path.len() >= 128 || !path.starts_with('/') {
            return Err("data root must be absolute and shorter than 128 bytes".into());
        }
        let root = c_string(path)?;
        unsafe {
            (*self.ctx).data_root = root;
        }
        Ok(())
    }

    /// A fresh state with the standard libraries Mosaico supports: base,
    /// package (preload only), coroutine, debug, math, string, table, utf8.
    pub fn new(
        host: Box<dyn Host>,
        limits: Limits,
        allocator: &'static RawAllocator,
        seed: u32,
    ) -> Result<Vm, String> {
        platform::set_allocator(allocator);
        let heap = Box::into_raw(Box::new(Heap {
            allocator,
            used: Cell::new(0),
            peak: Cell::new(0),
            limit: limits.memory_bytes,
            in_host: Cell::new(0),
        }));
        let default_root = critical_section::with(|cs| *DEFAULT_ROOT.borrow(cs).borrow());
        let root_len = default_root
            .iter()
            .position(|&b| b == 0)
            .expect("validated root");
        let ctx = Box::into_raw(Box::new(Ctx {
            data_root: default_root[..=root_len].to_vec(),
            host,
            heap,
            limits,
            thread: ptr::null_mut(),
            hooks: 0,
            since_yield: 0,
            slice_start: 0,
            last_yield: 0,
            interrupt: false,
            request: None,
            error_text: String::new(),
            error_arg: 0,
        }));
        unsafe {
            let main = lua_newstate(lua_alloc, heap.cast(), seed);
            if main.is_null() {
                drop(Box::from_raw(ctx));
                drop(Box::from_raw(heap));
                return Err("not enough memory for a Lua state".into());
            }
            let mut vm = Vm {
                main,
                thread: ptr::null_mut(),
                ctx,
                heap,
                state: State::Fresh,
            };
            mosaico_lua_set_ctx(main, ctx.cast());
            let _enter = Enter::new(ctx);
            if mosaico_lua_open(main) != LUA_OK {
                return Err(vm.pop_message(main));
            }
            let thread = mosaico_lua_new_thread(main);
            if thread.is_null() {
                return Err(vm.pop_message(main));
            }
            vm.thread = thread;
            (*ctx).thread = thread;
            let interval = limits.hook_interval.clamp(1, i32::MAX as u32) as c_int;
            mosaico_lua_install_hook(thread, interval);
            Ok(vm)
        }
    }

    /// Adds (or extends) the global table `module`, also visible through
    /// `require(module)`, with host functions.
    pub fn register(&mut self, module: &str, functions: &[(&str, Function)]) -> Result<(), String> {
        let mut names: Vec<Vec<u8>> = Vec::with_capacity(functions.len());
        for (name, _) in functions {
            names.push(cstring(name));
        }
        let regs: Vec<mosaico_lua_reg> = functions
            .iter()
            .zip(&names)
            .map(|((_, f), n)| mosaico_lua_reg {
                name: n.as_ptr().cast(),
                func: *f as *mut c_void,
            })
            .collect();
        let module = cstring(module);
        let _enter = Enter::new(self.ctx);
        let status = unsafe {
            mosaico_lua_register(self.main, module.as_ptr().cast(), regs.as_ptr(), regs.len())
        };
        if status == LUA_OK {
            Ok(())
        } else {
            Err(unsafe { self.pop_message(self.main) })
        }
    }

    /// Compiles the script (text only; precompiled chunks are refused).
    /// `name` appears in error messages as `name:line:`.
    pub fn load(&mut self, name: &str, source: &[u8]) -> Result<(), String> {
        if self.state != State::Fresh {
            return Err("a script is already loaded".into());
        }
        let mut chunk = Vec::with_capacity(name.len() + 2);
        chunk.push(b'@');
        chunk.extend_from_slice(name.as_bytes());
        chunk.push(0);
        let _enter = Enter::new(self.ctx);
        let status = unsafe {
            luaL_loadbufferx(
                self.thread,
                source.as_ptr().cast(),
                source.len(),
                chunk.as_ptr().cast(),
                b"t\0".as_ptr().cast(),
            )
        };
        if status != LUA_OK {
            self.state = State::Finished;
            return Err(unsafe { self.pop_message(self.thread) });
        }
        self.state = State::Loaded;
        Ok(())
    }

    /// Compiles the script at `path` (text only), as esp-claw's cap_lua does
    /// for an app entry: `require` searches the script's directory first,
    /// then `shared_dirs`, then the default `package.path`.
    pub fn load_file(&mut self, path: &str, shared_dirs: &[&str]) -> Result<(), String> {
        if self.state != State::Fresh {
            return Err("a script is already loaded".into());
        }
        let mut prefix = String::new();
        let dirs = path
            .rfind('/')
            .map(|i| &path[..i])
            .into_iter()
            .chain(shared_dirs.iter().copied());
        for dir in dirs {
            if !prefix.is_empty() {
                prefix.push(';');
            }
            prefix.push_str(&alloc::format!("{dir}/?.lua;{dir}/?/init.lua"));
        }
        let prefix = c_string(&prefix)?;
        let cpath = c_string(path)?;
        let _enter = Enter::new(self.ctx);
        if prefix.len() > 1
            && unsafe { mosaico_lua_prepend_path(self.main, prefix.as_ptr().cast()) } != LUA_OK
        {
            self.state = State::Finished;
            return Err(unsafe { self.pop_message(self.main) });
        }
        let status =
            unsafe { luaL_loadfilex(self.thread, cpath.as_ptr().cast(), b"t\0".as_ptr().cast()) };
        if status != LUA_OK {
            self.state = State::Finished;
            return Err(unsafe { self.pop_message(self.thread) });
        }
        self.state = State::Loaded;
        Ok(())
    }

    /// Runs the script until its slice ends, it waits, finishes or fails.
    pub fn resume(&mut self) -> Step {
        if self.state != State::Loaded {
            return Step::Done;
        }
        let ctx = unsafe { &mut *self.ctx };
        ctx.request = None;
        ctx.hooks = 0;
        if let Some(clock) = ctx.limits.clock {
            // `resume` only returns on a yield or the end, so every slice
            // starts right after a yield.
            ctx.slice_start = clock();
            ctx.last_yield = ctx.slice_start;
        }
        let mut nres: c_int = 0;
        let _enter = Enter::new(self.ctx);
        let status = unsafe { lua_resume(self.thread, ptr::null_mut(), 0, &mut nres) };
        let ctx = unsafe { &mut *self.ctx };
        match status {
            LUA_YIELD => {
                unsafe { lua_settop(self.thread, -nres - 1) };
                ctx.since_yield = 0;
                if ctx.interrupt {
                    self.state = State::Finished;
                    return Step::Interrupted;
                }
                match ctx.request.take() {
                    Some(request) => Step::Request(request),
                    None => Step::Slice,
                }
            }
            LUA_OK => {
                self.state = State::Finished;
                Step::Done
            }
            _ => {
                self.state = State::Finished;
                if ctx.interrupt {
                    return Step::Interrupted;
                }
                let message = unsafe {
                    if mosaico_lua_traceback(self.main, self.thread) == LUA_OK {
                        self.pop_message(self.main)
                    } else {
                        lua_settop(self.main, -2);
                        self.pop_message(self.thread)
                    }
                };
                Step::Error(message)
            }
        }
    }

    /// Stops the script at its next hook; the next [`Vm::resume`] returns
    /// [`Step::Interrupted`]. `pcall` in the script cannot catch this when
    /// the script can be yielded (the coroutine is abandoned instead).
    pub fn interrupt(&mut self) {
        unsafe { (*self.ctx).interrupt = true };
    }

    pub fn finished(&self) -> bool {
        self.state == State::Finished
    }

    /// Bytes currently allocated by Lua.
    pub fn memory_used(&self) -> usize {
        unsafe { (*self.heap).used.get() }
    }

    /// Highest [`Vm::memory_used`] so far.
    pub fn memory_peak(&self) -> usize {
        unsafe { (*self.heap).peak.get() }
    }

    /// Sets the `args` global from a JSON object, as esp-claw does for
    /// `launcher.json` args. Invalid JSON leaves `args` an empty table.
    pub fn set_args_json(&mut self, json: &str) -> Result<(), String> {
        let _enter = Enter::new(self.ctx);
        let status = unsafe { mosaico_lua_set_args(self.main, json.as_ptr().cast(), json.len()) };
        if status == LUA_OK {
            Ok(())
        } else {
            Err(unsafe { self.pop_message(self.main) })
        }
    }

    pub fn host_mut<T: Host>(&mut self) -> Option<&mut T> {
        let host: &mut dyn Any = unsafe { &mut *(*self.ctx).host };
        host.downcast_mut()
    }

    /// Pops the value on top of `l` as an error message.
    unsafe fn pop_message(&mut self, l: *mut lua_State) -> String {
        let text = to_string_lossy(l, -1).unwrap_or_else(|| {
            let tp = lua_type(l, -1);
            let mut s = String::from("(error object is a ");
            s.push_str(type_name(l, tp));
            s.push_str(" value)");
            s
        });
        lua_settop(l, -2);
        text
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        let _enter = Enter::new(self.ctx);
        unsafe {
            // Like cap_lua: modules release hardware (the screen) first.
            mosaico_lua_exit(self.main);
            lua_close(self.main);
            drop(Box::from_raw(self.ctx));
            drop(Box::from_raw(self.heap));
        }
    }
}

fn cstring(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend(s.bytes().filter(|&b| b != 0));
    v.push(0);
    v
}

pub(crate) unsafe fn to_string_lossy(l: *mut lua_State, idx: c_int) -> Option<String> {
    if lua_type(l, idx) != LUA_TSTRING {
        return None;
    }
    let mut len = 0usize;
    let p = lua_tolstring(l, idx, &mut len);
    let bytes = core::slice::from_raw_parts(p.cast::<u8>(), len);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

pub(crate) unsafe fn type_name(l: *mut lua_State, tp: c_int) -> &'static str {
    if tp == LUA_TNONE {
        return "no value";
    }
    let p = lua_typename(l, tp);
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    core::str::from_utf8(core::slice::from_raw_parts(p.cast::<u8>(), n)).unwrap_or("?")
}

// ---- Called from csrc/mosaico_lua_glue.c --------------------------------

#[no_mangle]
unsafe extern "C" fn mosaico_lua_on_hook(
    ctx: *mut c_void,
    l: *mut lua_State,
    yieldable: c_int,
) -> c_int {
    let ctx = &mut *(ctx as *mut Ctx);
    ctx.hooks = ctx.hooks.saturating_add(1);
    ctx.since_yield = ctx.since_yield.saturating_add(1);
    let can_yield = l == ctx.thread && yieldable != 0;
    if ctx.interrupt {
        return if can_yield { 1 } else { 2 };
    }
    if let Some(clock) = ctx.limits.clock {
        let now = clock();
        if can_yield && now.wrapping_sub(ctx.slice_start) >= ctx.limits.slice_us as u64 {
            return 1;
        }
        if now.wrapping_sub(ctx.last_yield) >= ctx.limits.max_blocked_us as u64 {
            return 3;
        }
        return 0;
    }
    if can_yield && ctx.hooks >= ctx.limits.slice_hooks {
        return 1;
    }
    if ctx.since_yield >= ctx.limits.max_blocked_hooks {
        return 3;
    }
    0
}

#[no_mangle]
unsafe extern "C" fn mosaico_lua_on_print(ctx: *mut c_void, s: *const c_char, len: usize) {
    let ctx = &mut *(ctx as *mut Ctx);
    ctx.host.print(core::slice::from_raw_parts(s.cast(), len));
}

#[no_mangle]
unsafe extern "C" fn mosaico_lua_call_rust(
    ctx: *mut c_void,
    l: *mut lua_State,
    f: *mut c_void,
) -> c_int {
    let ctx = &mut *(ctx as *mut Ctx);
    let heap = &*ctx.heap;
    let f: Function = core::mem::transmute::<*mut c_void, Function>(f);
    let can_yield = l == ctx.thread && lua_isyieldable(l) != 0;
    heap.in_host.set(heap.in_host.get() + 1);
    let result = {
        let mut call = Call::new(l, ctx, can_yield);
        f(&mut call)
    };
    heap.in_host.set(heap.in_host.get() - 1);
    match result {
        Ok(Ret::Values(n)) => n.max(0),
        Ok(Ret::Yield(request)) => {
            if can_yield {
                ctx.request = Some(request);
                -2
            } else {
                0 // inside a nested coroutine or C call: no wait
            }
        }
        Err(Error::Message(text)) => {
            ctx.error_text = text;
            -1
        }
        Err(Error::Arg { index, message }) => {
            ctx.error_text = message;
            ctx.error_arg = index;
            -3
        }
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_lua_error_text(
    ctx: *mut c_void,
    len: *mut usize,
    arg: *mut c_int,
) -> *const c_char {
    let ctx = &*(ctx as *const Ctx);
    *len = ctx.error_text.len();
    *arg = ctx.error_arg;
    ctx.error_text.as_ptr().cast()
}

static LOG_SINK: AtomicPtr<()> = AtomicPtr::new(ptr::null_mut());

const fn initial_root() -> [u8; 128] {
    let mut root = [0; 128];
    root[0] = b'/';
    root[1] = b'd';
    root[2] = b'a';
    root[3] = b't';
    root[4] = b'a';
    root
}
// Readers copy a fixed-size snapshot under the same short lock as writers.
// No pointer into this mutable default ever crosses the C boundary.
static DEFAULT_ROOT: critical_section::Mutex<core::cell::RefCell<[u8; 128]>> =
    critical_section::Mutex::new(core::cell::RefCell::new(initial_root()));

/// Set the validated default copied into subsequently created VMs.
/// Existing VMs retain their own root. The no-context C fallback is `/data`.
pub fn set_data_root(path: &str) -> Result<(), String> {
    if path.len() >= 128 || !path.starts_with('/') {
        return Err("data root must be absolute and shorter than 128 bytes".into());
    }
    let c = c_string(path)?;
    let mut root = [0; 128];
    root[..c.len()].copy_from_slice(&c);
    critical_section::with(|cs| *DEFAULT_ROOT.borrow(cs).borrow_mut() = root);
    Ok(())
}

/// A NUL-terminated copy of `s` (which must not contain NUL).
fn c_string(s: &str) -> Result<Vec<u8>, String> {
    if s.as_bytes().contains(&0) {
        return Err("path contains a NUL byte".into());
    }
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    Ok(v)
}

/// Where Lua's own warnings and panic messages go (the firmware log).
pub fn set_log_sink(sink: fn(&[u8])) {
    LOG_SINK.store(sink as *mut (), Ordering::Relaxed);
}

pub(crate) fn log_line(line: &[u8]) {
    let sink = LOG_SINK.load(Ordering::Relaxed);
    if !sink.is_null() {
        let sink: fn(&[u8]) = unsafe { core::mem::transmute::<*mut (), fn(&[u8])>(sink) };
        sink(line);
    }
}

#[no_mangle]
unsafe extern "C" fn mosaico_lua_log(s: *const c_char, len: usize) {
    log_line(core::slice::from_raw_parts(s.cast(), len));
}

/// A C module asks to suspend the script (1 sleep, 2 present).
#[no_mangle]
unsafe extern "C" fn mosaico_lua_on_request(
    ctx: *mut c_void,
    l: *mut lua_State,
    yieldable: c_int,
    kind: c_int,
    arg: u32,
) -> c_int {
    let ctx = &mut *(ctx as *mut Ctx);
    if l != ctx.thread || yieldable == 0 {
        return 0;
    }
    ctx.request = Some(match kind {
        1 => Request::Sleep { ms: arg },
        2 => Request::Present,
        other => Request::Host(other as u32),
    });
    1
}

#[no_mangle]
unsafe extern "C" fn mosaico_lua_on_stop(ctx: *mut c_void) -> c_int {
    (*(ctx as *const Ctx)).interrupt as c_int
}
