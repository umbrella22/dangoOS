//! Board peripherals and the low-level display plumbing.
//!
//! - [`display`]: CO5300 AMOLED over QSPI with DMA chunk uploads and TE sync.
//! - [`touch`]: CST9220 capacitive touch (report decoding in `touch/protocol.rs`).
//! - [`imu`]: BMI270 accelerometer/gyro and the BMM150 magnetometer pair.
//! - [`battery`]: BQ27220 fuel gauge.
//! - [`subboard`]: expansion slots: module detection on either side and the
//!   Interaction board inputs.
//! - [`shared_i2c`]: I2C0 shared by touch and the sensors, plus the I2C1
//!   subboard bus.
//! - [`geometry`]: panel size and the inclusive window type.
//! - [`memory`]: PSRAM-backed pixel buffers.
//! - [`lvgl_port`]: the headless LVGL display LVGL needs to create objects.

pub mod battery;
pub mod display;
pub mod geometry;
pub mod imu;
pub mod lvgl_port;
pub mod memory;
pub mod shared_i2c;
pub mod subboard;
pub mod touch;
