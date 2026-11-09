//! Shared physical drivers for the ESP-Mosaico S31 v1.1/v1.2 board.
#![no_std]
extern crate alloc;
#[cfg(target_os = "none")]
pub mod board;
#[cfg(target_os = "none")]
pub mod boot_display;
#[cfg(target_os = "none")]
pub mod display;
pub mod geometry;
#[cfg(target_os = "none")]
pub mod nand;
#[cfg(target_os = "none")]
pub mod reset;
#[cfg(target_os = "none")]
pub mod shared_i2c;
#[cfg(target_os = "none")]
pub mod touch;
