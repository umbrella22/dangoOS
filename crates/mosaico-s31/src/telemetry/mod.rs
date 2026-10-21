//! Counters for humans: touch-to-pixel latency and display throughput.
//!
//! Both are read by the firmware's once-per-second stats line and the console
//! boot record. Nothing here synchronizes anything.

pub mod display;
pub mod latency;
