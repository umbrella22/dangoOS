//! Shared application values and image codecs. This crate contains no script VM.
#![no_std]
extern crate alloc;
mod allocator;
#[cfg(feature = "graphics")]
pub mod graphics;
mod hardware;
#[cfg(feature = "graphics")]
pub mod image;
pub mod jpeg;
mod platform;
mod services;
pub mod ui;
pub use allocator::{RawAllocator, GLOBAL_ALLOCATOR};
pub use hardware::{ButtonEdge, HapticPreset, HardwareRequest, HardwareSnapshot, OnboardButtons};
pub use platform::{caps, esp_err, heap_fields, Blit, DisplayInfo, HeapInfo, TouchPoint};
pub use services::{
    AudioControl, AudioStatus, CameraStatus, NativeUiLimits, NetworkStatus, ServiceSnapshot,
};
