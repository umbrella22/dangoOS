//! Independent rescue image and its host-testable USB/storage contracts.
#![no_std]
extern crate alloc;
pub mod animation;
#[cfg(target_os = "none")]
pub mod device;
pub mod install;
pub mod protocol;
pub mod readonly;
pub mod server;
