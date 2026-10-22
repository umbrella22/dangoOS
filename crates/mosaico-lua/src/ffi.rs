//! Raw bindings for the parts of the Lua 5.5 C API and the glue we use.
//!
//! Hand-written (no bindgen): the set is small and stable. Types follow the
//! default `luaconf.h`: `lua_Integer` is `long long`, `lua_Number` is
//! `double`.

#![allow(non_camel_case_types, dead_code)]

use core::ffi::{c_char, c_int, c_void};

pub type lua_Integer = i64;
pub type lua_Number = f64;
pub type lua_Alloc = unsafe extern "C" fn(
    ud: *mut c_void,
    ptr: *mut c_void,
    osize: usize,
    nsize: usize,
) -> *mut c_void;

#[repr(C)]
pub struct lua_State {
    _private: [u8; 0],
}

pub const LUA_OK: c_int = 0;
pub const LUA_YIELD: c_int = 1;
pub const LUA_ERRRUN: c_int = 2;
pub const LUA_ERRSYNTAX: c_int = 3;
pub const LUA_ERRMEM: c_int = 4;

pub const LUA_TNONE: c_int = -1;
pub const LUA_TNIL: c_int = 0;
pub const LUA_TBOOLEAN: c_int = 1;
pub const LUA_TNUMBER: c_int = 3;
pub const LUA_TSTRING: c_int = 4;
pub const LUA_TTABLE: c_int = 5;

pub const LUA_GCCOUNT: c_int = 3;
pub const LUA_GCCOUNTB: c_int = 4;

/// One entry of a module table registered through the glue.
#[repr(C)]
pub struct mosaico_lua_reg {
    pub name: *const c_char,
    pub func: *mut c_void,
}

extern "C" {
    pub fn lua_newstate(f: lua_Alloc, ud: *mut c_void, seed: u32) -> *mut lua_State;
    pub fn lua_close(l: *mut lua_State);
    pub fn lua_newthread(l: *mut lua_State) -> *mut lua_State;
    pub fn lua_resume(
        l: *mut lua_State,
        from: *mut lua_State,
        narg: c_int,
        nres: *mut c_int,
    ) -> c_int;
    pub fn lua_gettop(l: *mut lua_State) -> c_int;
    pub fn lua_settop(l: *mut lua_State, idx: c_int);
    pub fn lua_type(l: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_typename(l: *mut lua_State, tp: c_int) -> *const c_char;
    pub fn lua_isinteger(l: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_tonumberx(l: *mut lua_State, idx: c_int, isnum: *mut c_int) -> lua_Number;
    pub fn lua_tointegerx(l: *mut lua_State, idx: c_int, isnum: *mut c_int) -> lua_Integer;
    pub fn lua_toboolean(l: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_tolstring(l: *mut lua_State, idx: c_int, len: *mut usize) -> *const c_char;
    pub fn lua_pushnil(l: *mut lua_State);
    pub fn lua_pushnumber(l: *mut lua_State, n: lua_Number);
    pub fn lua_pushinteger(l: *mut lua_State, n: lua_Integer);
    pub fn lua_pushlstring(l: *mut lua_State, s: *const c_char, len: usize) -> *const c_char;
    pub fn lua_pushboolean(l: *mut lua_State, b: c_int);
    pub fn lua_createtable(l: *mut lua_State, narr: c_int, nrec: c_int);
    pub fn lua_rawset(l: *mut lua_State, idx: c_int);
    pub fn lua_rawseti(l: *mut lua_State, idx: c_int, n: lua_Integer);
    pub fn lua_rotate(l: *mut lua_State, idx: c_int, n: c_int);
    pub fn lua_isyieldable(l: *mut lua_State) -> c_int;
    pub fn lua_gc(l: *mut lua_State, what: c_int, ...) -> c_int;

    pub fn luaL_loadbufferx(
        l: *mut lua_State,
        buff: *const c_char,
        sz: usize,
        name: *const c_char,
        mode: *const c_char,
    ) -> c_int;

    // csrc/mosaico_lua_glue.c
    pub fn mosaico_lua_set_ctx(l: *mut lua_State, ctx: *mut c_void);
    pub fn mosaico_lua_open(l: *mut lua_State) -> c_int;
    pub fn mosaico_lua_register(
        l: *mut lua_State,
        module: *const c_char,
        regs: *const mosaico_lua_reg,
        count: usize,
    ) -> c_int;
    pub fn mosaico_lua_new_thread(l: *mut lua_State) -> *mut lua_State;
    pub fn mosaico_lua_install_hook(l: *mut lua_State, count: c_int);
    pub fn mosaico_lua_traceback(l: *mut lua_State, thread: *mut lua_State) -> c_int;
}
