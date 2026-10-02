//! Compiled UI shared by both applications.
//! 按消费者分流（build.rs 同步选择编译根，避免两套 UI 同入固件）:
//!   std（桌面模拟器） → 旧风扇控制界面 FanControlUI + boot/history 模型
//!   embedded（固件）   → 官方风格 Hub UI MosaicApp
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(all(feature = "std", not(feature = "embedded")))]
pub mod boot;
#[cfg(all(feature = "std", not(feature = "embedded")))]
pub mod history;

slint::include_modules!();

pub const WIDTH: u32 = 480; // Mosaico 方屏（唯一布局）
pub const HEIGHT: u32 = 480;
