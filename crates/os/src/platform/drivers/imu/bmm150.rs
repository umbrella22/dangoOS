//! BMM150 discovery, trim compensation and magnetometer samples.
use super::{
    bus::{MagBus, MagReadMode},
    detection::probe_candidate,
    math::sqrt_f32,
};
use crate::platform::buses::shared_i2c;
use embassy_time::{Duration, Timer};
use log::{info, warn};

pub(super) const BMM150_CHIP_ID: u8 = 0x32;
// ---- BMM150 registers (Bosch BMM150 datasheet / SensorAPI) ----
pub(super) const BMM_CHIP_ID: u8 = 0x40;
pub(super) const BMM_DATA_X_LSB: u8 = 0x42;
pub(super) const BMM_DATA_READY_STATUS: u8 = 0x48;
pub(super) const BMM_POWER_CONTROL: u8 = 0x4B;
pub(super) const BMM_OP_MODE: u8 = 0x4C;
pub(super) const BMM_REP_XY: u8 = 0x51;
pub(super) const BMM_REP_Z: u8 = 0x52;
pub(super) const BMM_DIG_X1: u8 = 0x5D;
pub(super) const BMM_DIG_Z4_LSB: u8 = 0x62;
pub(super) const BMM_DIG_Z2_LSB: u8 = 0x68;

pub(super) const BMM_POWER_CNTRL_ENABLE: u8 = 0x01;
/// Bosch `BMM150_SET_SOFT_RESET` (`bmm150_defs.h`): write `0x4B` with
/// `0x80` (soft reset) | `0x02`. The BMI270 uses `0x7E = 0xB6`; that command
/// does not exist on the BMM150.
pub(super) const BMM_SOFT_RESET: u8 = 0x82;
/// OP_MODE: normal mode (bits 1:0 = 0) with ODR bits 5:3 = 0x05 (20 Hz).
pub(super) const BMM_OP_MODE_NORMAL_20HZ: u8 = 0x28;
/// Regular preset repetition counts.
pub(super) const BMM_REP_XY_REGULAR: u8 = 0x04;
pub(super) const BMM_REP_Z_REGULAR: u8 = 0x07;
pub(super) const BMM_DRDY_STATUS_MSK: u8 = 0x01;

/// First conversion timeout from BSP `sensors.c`.
pub(super) const BMM_DRDY_TIMEOUT_MS: u64 = 200;
pub(super) const BMM_DRDY_POLL_MS: u64 = 5;

/// BMM150 axes overflow values.
pub(super) const BMM_OVERFLOW_XY: i16 = -4096;
pub(super) const BMM_OVERFLOW_Z: i16 = -16384;

/// Probe order for BMM150 addresses.
///
/// `0x11`/`0x12` are the official BSP 7-bit addresses; `0x10`/`0x13` are the
/// remaining Bosch-documented addresses (`BMM150_DEFAULT_I2C_ADDRESS` and the
/// CSB/SDO combinations); `0x22`/`0x24` are kept because RFC 0004 and the
/// factory YAML name them (they are the 8-bit form of `0x11`/`0x12`).
pub(super) const BMM150_PROBE_ADDRS: [u8; 6] = [0x11, 0x12, 0x10, 0x13, 0x22, 0x24];

/// Factory trim values read once after power-up.
#[derive(Clone, Copy, Debug, Default)]
struct BmmTrim {
    dig_x1: i8,
    dig_y1: i8,
    dig_x2: i8,
    dig_y2: i8,
    dig_z1: u16,
    dig_z2: i16,
    dig_z3: i16,
    dig_z4: i16,
    dig_xy1: u8,
    dig_xy2: i8,
    dig_xyz1: u16,
}

/// One compensated magnetometer sample (microtesla).
#[derive(Clone, Copy, Debug, Default)]
pub struct MagSample {
    pub status: u8,
    pub x_ut: f32,
    pub y_ut: f32,
    pub z_ut: f32,
    /// Compensated vector magnitude in microtesla.
    pub magnitude_ut: f32,
}

/// The BMM150 driver.
pub struct Bmm150 {
    addr: u8,
    bus: MagBus,
    trim: BmmTrim,
    read_mode: MagReadMode,
}

impl Bmm150 {
    /// Probe the documented 7-bit addresses on both buses and return up to two
    /// devices. The official BSP places both sensors on I2C0; I2C1 is probed
    /// as a fallback and is used only when it answers with a valid
    /// BMM150 chip ID.
    ///
    /// The probe follows Bosch `bmm150_init`: power control `0x4B |= 0x01`,
    /// wait 3 ms, then read chip ID `0x40`. The plain read is tried first and
    /// the remaining recovery sequence (split STOP/STOP transfer, soft reset
    /// `0x4B |= 0x82`, 100 kHz) runs only if the ID is still not `0x32`.
    pub async fn probe_all() -> [Option<Self>; 2] {
        let mut found: [Option<Self>; 2] = [None, None];
        let mut slots_full = false;

        for bus in [MagBus::Main, MagBus::Subboard] {
            for &addr in &BMM150_PROBE_ADDRS {
                if slots_full {
                    break;
                }
                if found
                    .iter()
                    .flatten()
                    .any(|device| device.addr == addr && device.bus == bus)
                {
                    continue;
                }

                let Some(result) = probe_candidate(bus, addr).await else {
                    continue;
                };
                if result.chip_id == BMM150_CHIP_ID {
                    let slot = if found[0].is_none() {
                        0
                    } else if found[1].is_none() {
                        1
                    } else {
                        slots_full = true;
                        break;
                    };
                    found[slot] = Some(Self {
                        addr,
                        bus,
                        trim: BmmTrim::default(),
                        read_mode: result.read_mode,
                    });
                    info!(
                        "BMM150[{}] online on {} @0x{:02X} using {:?}",
                        slot,
                        bus.name(),
                        addr,
                        result.read_mode
                    );
                }
            }
        }

        if found.iter().all(Option::is_none) {
            warn!("BMM150: no compatible device found on I2C0 or I2C1");
        }
        found
    }

    pub fn address(&self) -> u8 {
        self.addr
    }

    pub fn bus(&self) -> MagBus {
        self.bus
    }

    /// Power up, read factory trim, select normal mode at 20 Hz with the
    /// regular preset, and wait for the first conversion.
    pub async fn configure(&mut self) -> Result<(), &'static str> {
        // Enable the power control bit (release suspend) and wait 3 ms.
        let mut pwr = [0u8; 1];
        self.read_regs(BMM_POWER_CONTROL, &mut pwr).await?;
        self.write_reg(BMM_POWER_CONTROL, pwr[0] | BMM_POWER_CNTRL_ENABLE)
            .await?;
        Timer::after(Duration::from_millis(3)).await;

        self.read_trim().await?;

        // Normal mode, ODR = 20 Hz (BMM150_ODR bits 5:3 = 0x05).
        self.write_reg(BMM_OP_MODE, BMM_OP_MODE_NORMAL_20HZ).await?;
        self.write_reg(BMM_REP_XY, BMM_REP_XY_REGULAR).await?;
        self.write_reg(BMM_REP_Z, BMM_REP_Z_REGULAR).await?;

        // Until the first conversion lands the data registers read zero, which
        // compensation turns into an overflow sample (BSP sensors.c).
        let mut ready = false;
        for _ in 0..=(BMM_DRDY_TIMEOUT_MS / BMM_DRDY_POLL_MS) {
            let mut status = [0u8; 1];
            self.read_regs(BMM_DATA_READY_STATUS, &mut status).await?;
            if status[0] & BMM_DRDY_STATUS_MSK != 0 {
                ready = true;
                break;
            }
            Timer::after(Duration::from_millis(BMM_DRDY_POLL_MS)).await;
        }
        if !ready {
            warn!(
                "BMM150 @{} 0x{:02X}: first sample not ready after {} ms",
                self.bus.name(),
                self.addr,
                BMM_DRDY_TIMEOUT_MS
            );
        }

        info!(
            "BMM150 ready @{} 0x{:02X}: normal mode, 20 Hz, regular preset",
            self.bus.name(),
            self.addr
        );
        Ok(())
    }

    /// Read one sample. Returns `None` when the data-ready bit is clear or the
    /// axes report the overflow codes.
    pub async fn read(&mut self) -> Result<Option<MagSample>, &'static str> {
        let mut data = [0u8; 8];
        self.read_regs(BMM_DATA_X_LSB, &mut data).await?;

        // Register 0x48 carries the DRDY bit in bit 0 and RHALL[5:0] in 7:2.
        if data[6] & BMM_DRDY_STATUS_MSK == 0 {
            return Ok(None);
        }

        let raw_x = ((data[1] as i8 as i16) * 32) | ((data[0] >> 3) as i16);
        let raw_y = ((data[3] as i8 as i16) * 32) | ((data[2] >> 3) as i16);
        let raw_z = ((data[5] as i8 as i16) * 128) | ((data[4] >> 1) as i16);
        let rhall = ((data[7] as u16) << 6) | ((data[6] >> 2) as u16);

        if raw_x == BMM_OVERFLOW_XY || raw_y == BMM_OVERFLOW_XY || raw_z == BMM_OVERFLOW_Z {
            return Ok(None);
        }

        let x_ut = self.compensate_xy(raw_x, rhall, true);
        let y_ut = self.compensate_xy(raw_y, rhall, false);
        let z_ut = self.compensate_z(raw_z, rhall);
        let magnitude_ut = sqrt_f32(x_ut * x_ut + y_ut * y_ut + z_ut * z_ut);

        let mut status = [0; 1];
        self.read_regs(0x4a, &mut status).await?;
        Ok(Some(MagSample {
            status: status[0],
            x_ut,
            y_ut,
            z_ut,
            magnitude_ut,
        }))
    }

    async fn read_trim(&mut self) -> Result<(), &'static str> {
        // Bosch reads three blocks: 0x5D/2, 0x62/4, 0x68/10.
        let mut x1y1 = [0u8; 2];
        self.read_regs(BMM_DIG_X1, &mut x1y1).await?;
        let mut xyz = [0u8; 4];
        self.read_regs(BMM_DIG_Z4_LSB, &mut xyz).await?;
        let mut xy1xy2 = [0u8; 10];
        self.read_regs(BMM_DIG_Z2_LSB, &mut xy1xy2).await?;

        self.trim = BmmTrim {
            dig_x1: x1y1[0] as i8,
            dig_y1: x1y1[1] as i8,
            dig_x2: xyz[2] as i8,
            dig_y2: xyz[3] as i8,
            dig_z1: u16::from_le_bytes([xy1xy2[2], xy1xy2[3]]),
            dig_z2: i16::from_le_bytes([xy1xy2[0], xy1xy2[1]]),
            dig_z3: i16::from_le_bytes([xy1xy2[6], xy1xy2[7]]),
            dig_z4: i16::from_le_bytes([xyz[0], xyz[1]]),
            dig_xy1: xy1xy2[9],
            dig_xy2: xy1xy2[8] as i8,
            dig_xyz1: u16::from_le_bytes([xy1xy2[4], xy1xy2[5] & 0x7F]),
        };
        Ok(())
    }

    /// Bosch `compensate_x` / `compensate_y` (float variant). The only
    /// difference is the trim pair and the output axis.
    fn compensate_xy(&self, mag: i16, rhall: u16, x_axis: bool) -> f32 {
        let trim = &self.trim;
        if mag == BMM_OVERFLOW_XY || rhall == 0 || trim.dig_xyz1 == 0 {
            return 0.0;
        }
        let (dig1, dig2) = if x_axis {
            (trim.dig_x1, trim.dig_x2)
        } else {
            (trim.dig_y1, trim.dig_y2)
        };

        let comp0 = trim.dig_xyz1 as f32 * 16384.0 / rhall as f32;
        let retval = comp0 - 16384.0;
        let comp1 = trim.dig_xy2 as f32 * (retval * retval / 268_435_456.0);
        let comp2 = comp1 + retval * trim.dig_xy1 as f32 / 16384.0;
        let comp3 = dig2 as f32 + 160.0;
        let comp4 = mag as f32 * ((comp2 + 256.0) * comp3);
        ((comp4 / 8192.0) + (dig1 as f32 * 8.0)) / 16.0
    }

    /// Bosch `compensate_z` (float variant).
    fn compensate_z(&self, mag: i16, rhall: u16) -> f32 {
        let trim = &self.trim;
        if mag == BMM_OVERFLOW_Z
            || trim.dig_z1 == 0
            || trim.dig_z2 == 0
            || trim.dig_xyz1 == 0
            || rhall == 0
        {
            return 0.0;
        }

        let z0 = mag as f32 - trim.dig_z4 as f32;
        let z1 = rhall as f32 - trim.dig_xyz1 as f32;
        let z2 = trim.dig_z3 as f32 * z1;
        let z3 = trim.dig_z1 as f32 * rhall as f32 / 32768.0;
        let z4 = trim.dig_z2 as f32 + z3;
        if z4 == 0.0 {
            return 0.0;
        }
        let z5 = z0 * 131_072.0 - z2;
        (z5 / (z4 * 4.0)) / 16.0
    }

    async fn write_reg(&self, reg: u8, value: u8) -> Result<(), &'static str> {
        let result = match (self.bus, self.read_mode) {
            (MagBus::Subboard, _) => {
                shared_i2c::subboard_write_reg_async(self.addr, reg, value).await
            }
            (MagBus::Main, MagReadMode::RepeatedStart | MagReadMode::Split) => {
                shared_i2c::write_reg_async(self.addr, reg, value).await
            }
            (MagBus::Main, MagReadMode::RepeatedStart100k | MagReadMode::Split100k) => {
                shared_i2c::write_reg_at_khz_async(self.addr, reg, value, 100).await
            }
        };
        match result {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("BMM150 I2C write"),
            None => Err("shared I2C not installed"),
        }
    }

    async fn read_regs(&self, reg: u8, data: &mut [u8]) -> Result<(), &'static str> {
        let result = match (self.bus, self.read_mode) {
            (MagBus::Subboard, _) => {
                shared_i2c::subboard_read_regs_async(self.addr, reg, data).await
            }
            (MagBus::Main, MagReadMode::RepeatedStart) => {
                shared_i2c::read_regs_async(self.addr, reg, data).await
            }
            (MagBus::Main, MagReadMode::Split) => {
                shared_i2c::read_regs_split_async(self.addr, reg, data).await
            }
            (MagBus::Main, MagReadMode::RepeatedStart100k) => {
                shared_i2c::read_regs_at_khz_async(self.addr, reg, data, 100).await
            }
            (MagBus::Main, MagReadMode::Split100k) => {
                shared_i2c::read_regs_split_at_khz_async(self.addr, reg, data, 100).await
            }
        };
        match result {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("BMM150 I2C read"),
            None => Err("shared I2C not installed"),
        }
    }
}
