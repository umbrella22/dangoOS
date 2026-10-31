//! I2C0 sensor cluster: BMI270 accelerometer/gyroscope and BMM150
//! magnetometers.
//!
//! ## Sources
//!
//! - BMI270 initialization, the 8192-byte Bosch configuration blob and the
//!   register sequence are ported from
//!   `archive/crates-pre-s31-20261019/fan-firmware/src/drivers/imu/protocol.rs`
//!   (Bosch BMI270 SensorAPI v2.86.1). The blob is copied beside this file
//!   (`src/assets/bmi270-config.bin`, SHA-256
//!   `2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db`).
//! - The official BSP `components/esp-mosaico-bsp/onboard/sensors.c` requires
//!   the configuration load, `bmi2_get_sensor_data` reads and the BMM150
//!   data-ready poll (5 ms interval, 200 ms timeout) before the first sample.
//! - BMM150 register values and compensation come from Bosch
//!   `BMM150_SensorAPI` (`bmm150.c` / `bmm150_defs.h`).
//!
//! ## Addresses
//!
//! esp-hal takes 7-bit addresses. The official BSP defines BMI270 `0x69` and
//! BMM150 `0x11` / `0x12` (`bsp/esp_mosaico.h`, `BSP_BMM150_ADDR_0/1`); the
//! factory `board_devices.yaml` writes the same pair as 8-bit `0x22` / `0x24`
//! and `setup_magnetometer.c` shifts it right by one. Bosch documents `0x10`,
//! `0x11`, `0x12` and `0x13` as the BMM150 I2C address set. The drivers
//! therefore *probe* the documented candidates and keep the address that
//! answers with chip ID `0x32`. No address is assumed.
//!
//! Per BSP `onboard/sensors.c`, both BMM150s are on the **shared main I2C0**
//! (`BSP_I2C_PORT = I2C_NUM_0`, v1.2 SDA `GPIO56` / SCL `GPIO3`), together
//! with the BMI270 and CST9220. `sensor_task` also probes the right-slot
//! subboard I2C1 as a fallback; the module EEPROM continues to use that bus.
//!
//! ## Rates
//!
//! Both sensors are configured for 50 Hz accelerometer/gyroscope and 20 Hz
//! magnetometer output. `sensor_task` polls at 50 Hz and publishes only new
//! samples.

mod bmi270;
mod bmm150;
mod bus;
mod detection;
mod math;

pub use bmi270::{Bmi270, ImuSample, FIFO_MAX_FRAMES};
pub use bmm150::{Bmm150, MagSample};
