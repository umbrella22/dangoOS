//! Host functions: Rust code called from Lua.
//!
//! A [`Function`] reads its arguments and pushes its results through
//! [`Call`]. It reports failures by returning [`Error`]; the C trampoline
//! raises the Lua error after the Rust frame has returned, so no `longjmp`
//! ever crosses Rust code. The cap from [`crate::Limits::memory_bytes`] is
//! not enforced while a host function runs, so its pushes only fail on real
//! memory exhaustion (which aborts, like any Rust allocation failure).

use alloc::string::String;
use core::{any::Any, ffi::c_int};

use crate::ffi::*;
use crate::vm::{type_name, Ctx, Host, Request};

/// A host function. Register it with [`crate::Vm::register`].
pub type Function = fn(&mut Call) -> Result<Ret, Error>;

/// What a host function returns to Lua.
#[derive(Debug, PartialEq, Eq)]
pub enum Ret {
    /// This many values pushed with `push_*`.
    Values(i32),
    /// Suspend the script. Only honoured when [`Call::can_yield`] is true;
    /// otherwise the function returns no values and the script continues.
    Yield(Request),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Raised as `chunk:line: message`.
    Message(String),
    /// Raised as `bad argument #index to 'name' (message)`.
    Arg { index: i32, message: String },
}

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Error::Message(text.into())
    }
    pub fn arg(index: i32, message: impl Into<String>) -> Self {
        Error::Arg {
            index,
            message: message.into(),
        }
    }
}

pub struct Call<'a> {
    l: *mut lua_State,
    ctx: &'a mut Ctx,
    can_yield: bool,
}

impl<'a> Call<'a> {
    pub(crate) fn new(l: *mut lua_State, ctx: &'a mut Ctx, can_yield: bool) -> Self {
        Self { l, ctx, can_yield }
    }

    /// The host object passed to [`crate::Vm::new`].
    pub fn host<T: Host>(&mut self) -> Option<&mut T> {
        let host: &mut dyn Any = &mut *self.ctx.host;
        host.downcast_mut()
    }

    /// True when a [`Ret::Yield`] would suspend the script (it runs on the
    /// script's own coroutine and not inside a C call).
    pub fn can_yield(&self) -> bool {
        self.can_yield
    }

    pub fn arg_count(&self) -> i32 {
        unsafe { lua_gettop(self.l) }
    }

    pub fn is_none_or_nil(&self, index: i32) -> bool {
        unsafe { lua_type(self.l, index) <= LUA_TNIL }
    }

    fn type_error(&self, index: i32, expected: &str) -> Error {
        let got = unsafe { type_name(self.l, lua_type(self.l, index)) };
        let mut m = String::from(expected);
        m.push_str(" expected, got ");
        m.push_str(got);
        Error::arg(index, m)
    }

    pub fn number(&self, index: i32) -> Result<f64, Error> {
        let mut ok: c_int = 0;
        let n = unsafe { lua_tonumberx(self.l, index, &mut ok) };
        if ok != 0 {
            Ok(n)
        } else {
            Err(self.type_error(index, "number"))
        }
    }

    pub fn opt_number(&self, index: i32, default: f64) -> Result<f64, Error> {
        if self.is_none_or_nil(index) {
            Ok(default)
        } else {
            self.number(index)
        }
    }

    pub fn integer(&self, index: i32) -> Result<i64, Error> {
        let mut ok: c_int = 0;
        let n = unsafe { lua_tointegerx(self.l, index, &mut ok) };
        if ok != 0 {
            Ok(n)
        } else if unsafe { lua_type(self.l, index) } == LUA_TNUMBER {
            Err(Error::arg(index, "number has no integer representation"))
        } else {
            Err(self.type_error(index, "number"))
        }
    }

    pub fn opt_integer(&self, index: i32, default: i64) -> Result<i64, Error> {
        if self.is_none_or_nil(index) {
            Ok(default)
        } else {
            self.integer(index)
        }
    }

    pub fn boolean(&self, index: i32) -> bool {
        unsafe { lua_toboolean(self.l, index) != 0 }
    }

    /// String argument (numbers are converted, as in `luaL_checklstring`).
    pub fn bytes(&self, index: i32) -> Result<&[u8], Error> {
        let tp = unsafe { lua_type(self.l, index) };
        if tp != LUA_TSTRING && tp != LUA_TNUMBER {
            return Err(self.type_error(index, "string"));
        }
        let mut len = 0usize;
        let p = unsafe { lua_tolstring(self.l, index, &mut len) };
        Ok(unsafe { core::slice::from_raw_parts(p.cast(), len) })
    }

    pub fn str(&self, index: i32) -> Result<&str, Error> {
        core::str::from_utf8(self.bytes(index)?).map_err(|_| Error::arg(index, "invalid UTF-8"))
    }

    pub fn push_nil(&mut self) {
        unsafe { lua_pushnil(self.l) }
    }
    pub fn push_number(&mut self, n: f64) {
        unsafe { lua_pushnumber(self.l, n) }
    }
    pub fn push_integer(&mut self, n: i64) {
        unsafe { lua_pushinteger(self.l, n) }
    }
    pub fn push_bool(&mut self, b: bool) {
        unsafe { lua_pushboolean(self.l, b as c_int) }
    }
    pub fn push_bytes(&mut self, s: &[u8]) {
        unsafe {
            lua_pushlstring(self.l, s.as_ptr().cast(), s.len());
        }
    }
    pub fn push_str(&mut self, s: &str) {
        self.push_bytes(s.as_bytes())
    }

    /// Pushes a new table; fill it with the `set_*` methods.
    pub fn push_table(&mut self, array: i32, fields: i32) {
        unsafe { lua_createtable(self.l, array, fields) }
    }

    /// `t[key] = <value on top>` for the table just below the top, popping
    /// the value. Raw set: no metamethods run.
    fn set_top(&mut self, key: &str) {
        unsafe {
            // stack: t v -> t v k -> t k v ... rawset needs t k v
            lua_pushlstring(self.l, key.as_ptr().cast(), key.len());
            lua_rotate(self.l, -2, 1);
            lua_rawset(self.l, -3);
        }
    }
    pub fn set_integer(&mut self, key: &str, n: i64) {
        self.push_integer(n);
        self.set_top(key);
    }
    pub fn set_number(&mut self, key: &str, n: f64) {
        self.push_number(n);
        self.set_top(key);
    }
    pub fn set_bool(&mut self, key: &str, b: bool) {
        self.push_bool(b);
        self.set_top(key);
    }
    pub fn set_str(&mut self, key: &str, s: &str) {
        self.push_str(s);
        self.set_top(key);
    }
}
