//! Restricted resident Lua behavior with separate host interface, VM and controller.
//! Callback budgets and transactional replacement stay independent of foreground Lua.
mod controller;
mod interface;
#[cfg(test)]
mod tests;
mod vm;

pub use controller::Controller;
pub use interface::{expression, Output};
pub use vm::BrainVm;

pub const DEFAULT_SOURCE: &str = include_str!("../../../assets/brain/default.lua");
pub const SOURCE_MAX: usize = 64 * 1024;
pub const HEAP_MAX: usize = 512 * 1024;
pub const CALLBACK_HOOKS: u32 = 2_000; // 200,000 instructions, across slices/pcall
pub const CALLBACK_US: u64 = 20_000;

#[cfg(target_os = "none")]
pub(crate) mod control;

pub mod store;
