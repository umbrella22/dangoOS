//! Main/subboard bus selection and supported magnetometer transfer forms.
use crate::platform::buses::shared_i2c;
use esp_hal::i2c::master::Error as I2cError;

/// Which controller a magnetometer is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MagBus {
    /// Main I2C0 (SDA GPIO56 / SCL GPIO3), shared with BMI270 and touch.
    Main,
    /// Right-slot subboard I2C1 (SDA GPIO0 / SCL GPIO1).
    Subboard,
}

impl MagBus {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Main => "I2C0",
            Self::Subboard => "I2C1",
        }
    }
}

/// Which I2C transfer form actually returns a valid BMM150 chip ID.
///
/// Bosch / ESP-IDF use a repeated start (register write, SR, read). If a
/// board-level timing issue makes that return `0x00`, the probe ladder tries
/// the other forms and keeps the one that works for normal operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MagReadMode {
    /// Register write + repeated start + read at 400 kHz.
    RepeatedStart,
    /// Register write with STOP, then a separate read at 400 kHz.
    Split,
    /// Register write + repeated start + read at 100 kHz.
    RepeatedStart100k,
    /// Two independent transactions at 100 kHz.
    Split100k,
}

impl MagReadMode {
    pub(super) const fn khz(self) -> u32 {
        match self {
            Self::RepeatedStart | Self::Split => 400,
            Self::RepeatedStart100k | Self::Split100k => 100,
        }
    }
}

pub(super) async fn bus_read_regs(
    bus: MagBus,
    addr: u8,
    reg: u8,
    data: &mut [u8],
) -> Option<Result<(), I2cError>> {
    match bus {
        MagBus::Main => shared_i2c::read_regs_async(addr, reg, data).await,
        MagBus::Subboard => shared_i2c::subboard_read_regs_async(addr, reg, data).await,
    }
}

pub(super) async fn bus_write_reg(
    bus: MagBus,
    addr: u8,
    reg: u8,
    value: u8,
) -> Option<Result<(), I2cError>> {
    match bus {
        MagBus::Main => shared_i2c::write_reg_async(addr, reg, value).await,
        MagBus::Subboard => shared_i2c::subboard_write_reg_async(addr, reg, value).await,
    }
}
