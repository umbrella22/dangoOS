//! Board peripheral drivers (CO5300 display, CST9220 touch, shared I2C,
//! BMI270/BMM150 sensors, BQ27220 fuel gauge, right-slot module).

pub mod battery;
pub mod display;
pub mod imu;
pub mod module;
pub mod shared_i2c;
pub mod touch;
