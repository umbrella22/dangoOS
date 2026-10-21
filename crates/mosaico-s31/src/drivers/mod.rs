//! Board peripherals and the low-level display plumbing.
//!
//! - [`display`]: CO5300 AMOLED over QSPI with DMA chunk uploads and TE sync.
//! - [`touch`]: CST9220 capacitive touch (report decoding in `touch/protocol.rs`).
//! - [`imu`]: BMI270 accelerometer/gyro and the BMM150 magnetometer pair.
//! - [`battery`]: BQ27220 fuel gauge.
//! - [`module`]: right-slot module detection over the sub-board EEPROM.
//! - [`shared_i2c`]: I2C0 shared by touch and the sensors.
//! - [`geometry`]: panel size and the inclusive window type.
//! - [`memory`]: PSRAM-backed pixel buffers.
//! - [`lvgl_port`]: the headless LVGL display LVGL needs to create objects.

pub mod battery;
pub mod display;
pub mod geometry;
pub mod imu;
pub mod lvgl_port;
pub mod memory;
pub mod module;
pub mod shared_i2c;
pub mod touch;
