//! BQ27220 fuel gauge on the main I2C0 bus.
//!
//! ## Hardware facts
//!
//! - The v1.2 board carries an 80 mAh single cell plus a BQ27220 gauge at
//!   7-bit `0x55` (`docs/MOSAICO_V12_HARDWARE.md` §3, BSP
//!   `bsp_mosaico.h` `BSP_BATTERY_I2C_ADDR`).
//! - The gauge is on the **shared main I2C0** together with the CST9220 and
//!   the Bosch sensors, so every access goes through
//!   [`crate::drivers::shared_i2c`] and inherits its 100 ms transaction
//!   timeout. A gauge read is two short transactions and cannot starve touch.
//! - The official component (`espressif/bq27220`) initializes by writing a
//!   CEDV profile and then re-sealing the gauge. Sealing needs the *unseal*
//!   key sequence and a full CEDV write, which requires the measured cell
//!   parameters. This driver only **reads**, so it never changes the gauge
//!   configuration: a read-only gauge keeps the factory profile intact and
//!   cannot brick the pack.
//!
//! ## Registers used
//!
//! The BQ27220 (and the whole bq27xxx family) exposes 16-bit standard-command
//! words at **2-byte-aligned addresses**, which is why the gaps below are not
//! contiguous. The map was verified against device data on 2026-10-21: the
//! pack reported `100%` state of charge, `0x06 = 3021` (302.1 K = 29 °C) and
//! `0x08 = 4028` (4.028 V), which is only consistent with temperature at
//! `0x06` and voltage at `0x08`.
//!
//! | Address | Field | Notes |
//! | --- | --- | --- |
//! | `0x00` | `Control()` | sub-commands (MAC / standard commands) |
//! | `0x02` | `AtRate()` | not read |
//! | `0x04` | reserved on this part | |
//! | `0x06` | `Temperature()` | 0.1 K |
//! | `0x08` | `Voltage()` | mV |
//! | `0x0A` | `BatteryStatus()` | bq27xxx flag word |
//! | `0x0C` | `RemainingCapacity()` | mAh |
//! | `0x0E` | `FullChargeCapacity()` | mAh |
//! | `0x12` | `FullChargeCapacity()` alternate word | not read |
//! | `0x14` | `AverageCurrent()` | signed mA |
//! | `0x2C` | `StateOfCharge()` | percent |
//!
//! The instantaneous `Current()` word is not in the bq27xxx standard set; the
//! pack only exposes `AverageCurrent()` at `0x14`, so that is what a sample
//! reports. It is an average over the gauge's internal window, not an
//! instantaneous reading, and the field name says so.
//!
//! The 16-bit registers are little-endian.
//!
//! ## Failure policy
//!
//! A gauge that does not answer (unpopulated pack, bus error, sealed gauge
//! refusing a read) disables battery reporting instead of failing the sensor
//! task. `probe` reads `StateOfCharge` once; any error or a value above 100%
//! means "no usable gauge" and the runtime keeps the normal resting face.

use core::fmt::Write as _;

use log::{info, warn};

use crate::drivers::shared_i2c;

/// BQ27220 7-bit address.
pub const ADDRESS: u8 = 0x55;

/// Standard-command `StateOfCharge()`.
const REG_STATE_OF_CHARGE: u8 = 0x2C;
/// Standard-command `Voltage()`.
const REG_VOLTAGE: u8 = 0x08;
/// Standard-command `AverageCurrent()` (signed).
const REG_AVERAGE_CURRENT: u8 = 0x14;
/// Standard-command `Temperature()` (0.1 K).
const REG_TEMPERATURE: u8 = 0x06;
/// Standard-command `BatteryStatus()`.
const REG_BATTERY_STATUS: u8 = 0x0A;

/// One decoded gauge sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatterySample {
    /// State of charge, `0..=100` percent.
    pub percent: u8,
    /// Pack voltage in millivolts.
    pub voltage_mv: u16,
    /// Gauge average current in mA; positive while charging.
    pub average_current_ma: i16,
    /// Pack temperature in tenths of a kelvin.
    pub temperature_dk: u16,
    /// `BatteryStatus()` flag word.
    pub status: u16,
}

impl BatterySample {
    /// Temperature in degrees Celsius.
    pub fn temperature_c(&self) -> i16 {
        self.temperature_dk as i16 / 10 - 273
    }
}

/// A probed gauge.
pub struct Bq27220;

impl Bq27220 {
    /// Probe the gauge by reading `StateOfCharge()`.
    ///
    /// Returns `None` when the gauge does not answer or reports an impossible
    /// state of charge. The probe is intentionally one short read: a pack
    /// without a gauge must not delay sensor bring-up.
    pub async fn probe() -> Option<Self> {
        let percent = read_u16(REG_STATE_OF_CHARGE).await;
        match percent {
            Ok(value) if value <= 100 => {
                info!("BQ27220 @0x{ADDRESS:02X}: gauge online, soc={value}%");
                // One read-only dump of the standard-command words, so the map
                // can be verified from the console against real values (a
                // voltage near 4000 mV, a temperature near 3000 in 0.1 K, a
                // state of charge in 0..=100). Nothing is written.
                dump_standard_commands().await;
                Some(Self)
            }
            Ok(value) => {
                warn!("BQ27220 @0x{ADDRESS:02X}: implausible soc={value}%, battery reporting disabled");
                None
            }
            Err(error) => {
                info!("BQ27220 @0x{ADDRESS:02X}: no gauge ({error}), battery reporting disabled");
                None
            }
        }
    }

    /// Read one full sample.
    ///
    /// Each register is a separate two-byte transaction; a failed read fails
    /// the whole sample instead of mixing stale fields.
    pub async fn read(&mut self) -> Result<BatterySample, &'static str> {
        let percent = read_u16(REG_STATE_OF_CHARGE).await?;
        if percent > 100 {
            return Err("state of charge out of range");
        }
        let voltage_mv = read_u16(REG_VOLTAGE).await?;
        let average_current_ma = read_u16(REG_AVERAGE_CURRENT).await? as i16;
        let temperature_dk = read_u16(REG_TEMPERATURE).await?;
        let status = read_u16(REG_BATTERY_STATUS).await?;
        Ok(BatterySample {
            percent: percent as u8,
            voltage_mv,
            average_current_ma,
            temperature_dk,
            status,
        })
    }
}

/// Log the standard-command words once, eight per line.
///
/// Read-only: a gauge that refuses a word simply logs `----`.
async fn dump_standard_commands() {
    const WORDS: [u8; 20] = [
        0x00, 0x02, 0x04, 0x06, 0x08, 0x0A, 0x0C, 0x0E, 0x10, 0x12, 0x14, 0x16, 0x18, 0x1A, 0x1C,
        0x1E, 0x24, 0x28, 0x2A, 0x2C,
    ];

    let mut line = ArrayString::new();
    for reg in WORDS {
        if line.len() + 12 > LOG_WIDTH && line.len() > 0 {
            info!("BQ27220 dump: {line}");
            line.clear();
        }
        match read_u16(reg).await {
            Ok(value) => {
                let _ = write!(line, "{reg:02X}={value:04X} ");
            }
            Err(_) => {
                let _ = write!(line, "{reg:02X}=---- ");
            }
        }
    }
    if line.len() > 0 {
        info!("BQ27220 dump: {line}");
    }
}

/// Small stack-only string used by the dump formatter.
struct ArrayString {
    bytes: [u8; LOG_WIDTH],
    len: usize,
}

impl ArrayString {
    const fn new() -> Self {
        Self {
            bytes: [0; LOG_WIDTH],
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn clear(&mut self) {
        self.len = 0;
    }
}

impl core::fmt::Write for ArrayString {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = self.bytes.len().saturating_sub(self.len);
        let count = core::cmp::min(available, text.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

impl core::fmt::Display for ArrayString {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(core::str::from_utf8(&self.bytes[..self.len]).unwrap_or(""))
    }
}

/// Width budget for one dump line; the console record is 158 text bytes.
const LOG_WIDTH: usize = 120;

async fn read_u16(reg: u8) -> Result<u16, &'static str> {
    let mut bytes = [0u8; 2];
    match shared_i2c::read_regs_async(ADDRESS, reg, &mut bytes).await {
        Some(Ok(())) => Ok(u16::from_le_bytes(bytes)),
        Some(Err(_)) => Err("i2c error"),
        None => Err("bus not installed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temperature_conversion_is_celsius() {
        // 3021 * 0.1 K = 302.1 K = 29 C, matching the 2026-10-21 device log.
        let sample = BatterySample {
            percent: 100,
            voltage_mv: 4_028,
            average_current_ma: -120,
            temperature_dk: 3_021,
            status: STATUS_DSG,
        };
        assert_eq!(sample.temperature_c(), 29);
        assert!(sample.is_discharging());
        assert!(!sample.is_full());
    }

    #[test]
    fn status_bits_match_the_bq27xxx_flag_word() {
        assert_eq!(STATUS_DSG, 0x0001);
        assert_eq!(STATUS_FC, 0x0200);
    }

    #[test]
    fn the_register_words_are_two_byte_aligned() {
        // bq27xxx standard commands live at even addresses; an odd address
        // would silently read the neighbouring word.
        for reg in [
            REG_STATE_OF_CHARGE,
            REG_VOLTAGE,
            REG_AVERAGE_CURRENT,
            REG_TEMPERATURE,
            REG_BATTERY_STATUS,
        ] {
            assert_eq!(reg % 2, 0, "register 0x{reg:02X} is not word aligned");
        }
    }
}
