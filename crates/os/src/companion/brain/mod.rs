//! Restricted resident Lua behavior with separate host interface, VM and controller.
//! Callback budgets and transactional replacement stay independent of foreground Lua.
mod controller;
mod input;
#[cfg(any(feature = "lua-engine", all(test, feature = "ts-engine")))]
mod interface;
#[cfg(all(test, feature = "lua-engine"))]
mod tests;
#[cfg(feature = "lua-engine")]
mod vm;

pub use controller::Controller;
#[cfg(feature = "lua-engine")]
pub use interface::expression;
mod output;
pub use output::Output;
#[cfg(feature = "lua-engine")]
pub use vm::BrainVm;

#[cfg(feature = "lua-engine")]
pub const DEFAULT_SOURCE: &str = include_str!("../../../assets/brain/default.lua");
pub const SOURCE_MAX: usize = 64 * 1024;
pub const HEAP_MAX: usize = 512 * 1024;
pub const CALLBACK_HOOKS: u32 = 2_000; // 200,000 instructions, across slices/pcall
pub const CALLBACK_US: u64 = 20_000;

#[cfg(target_os = "none")]
pub(crate) mod control;

pub mod store;

#[cfg(feature = "ts-engine")]
pub(crate) mod ts_vm;
#[cfg(feature = "ts-engine")]
pub use ts_vm::expression;
#[cfg(feature = "ts-engine")]
pub use ts_vm::BrainVm;
#[cfg(feature = "ts-engine")]
pub const DEFAULT_SOURCE: &str =
    include_str!("../../../../../artifacts/embedts/brain/default.etb.hex");

#[cfg(all(test, feature = "ts-engine"))]
#[path = "vm.rs"]
mod lua_oracle;
#[cfg(all(test, feature = "ts-engine"))]
mod ts_tests;
