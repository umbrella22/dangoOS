//! Lua 5.5 for ESP-Mosaico.
//!
//! The PUC-Rio Lua 5.5.1 core (vendored, MIT) compiled with `cc`, plus a
//! small C glue and a Rust API built around one rule: Lua errors are raised
//! only from C frames. See `docs/plans/lua-runtime-plan.md`.
//!
//! - [`Vm`]: one state, one script, cooperative time slices.
//! - [`Call`]/[`Function`]: host functions written in Rust.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod call;
mod ffi;
mod vm;

pub use call::{Call, Error, Function, Ret};
pub use vm::{set_log_sink, Host, Limits, RawAllocator, Request, Step, Vm, GLOBAL_ALLOCATOR};

/// `_VERSION` of the vendored interpreter plus its release number.
pub const LUA_RELEASE: &str = "Lua 5.5.1";

#[cfg(test)]
mod tests;
