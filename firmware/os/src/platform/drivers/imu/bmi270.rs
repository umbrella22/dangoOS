//! BMI270 configuration, FIFO and physical-unit samples.
use super::math::scale_i16;
use crate::platform::buses::shared_i2c;
use embassy_time::{Duration, Timer};
use log::{info, warn};

/// Unmodified Bosch `bmi270_config_file` (8192 bytes).
const BMI270_CONFIG: &[u8; 8192] = include_bytes!("../../../assets/bmi270-config.bin");

const BMI270_CHIP_ID: u8 = 0x24;

// ---- BMI270 registers (Bosch BMI270 datasheet / SensorAPI) ----
const REG_CHIP_ID: u8 = 0x00;
const REG_STATUS: u8 = 0x03;
const REG_INTERNAL_STATUS: u8 = 0x21;
const REG_ACC_CONF: u8 = 0x40;
const REG_ACC_RANGE: u8 = 0x41;
const REG_GYR_CONF: u8 = 0x42;
const REG_GYR_RANGE: u8 = 0x43;
const REG_INIT_CTRL: u8 = 0x59;
const REG_INIT_ADDR_0: u8 = 0x5B;
const REG_INIT_DATA: u8 = 0x5E;
const REG_PWR_CONF: u8 = 0x7C;
const REG_PWR_CTRL: u8 = 0x7D;
const REG_CMD: u8 = 0x7E;
const CMD_SOFT_RESET: u8 = 0xB6;

/// ACC_CONF: performance filter, NORMAL_AVG4, 50 Hz.
/// `BMI2_ACC_ODR_50HZ` is `0x07` (not `0x06`, which is 25 Hz).
const ACC_CONF_50HZ: u8 = 0xA7;
/// ACC_RANGE: ±2 g.
const ACC_RANGE_2G: u8 = 0x00;
/// GYR_CONF: performance filter, power-optimized noise, normal BWP, 50 Hz.
/// `BMI2_GYR_ODR_50HZ` is `0x07`.
const GYR_CONF_50HZ: u8 = 0xA7;
/// GYR_RANGE: ±2000 °/s.
const GYR_RANGE_2000DPS: u8 = 0x00;
/// PWR_CTRL: accel + gyro enabled.
const PWR_CTRL_ACC_GYR: u8 = 0x06;

/// One BMI270 burst: status (0x03) through the gyro Z MSB (0x17).
const IMU_BURST_LEN: usize = 21;
const IMU_STATUS_ACC_READY: u8 = 0x80;
const IMU_STATUS_GYR_READY: u8 = 0x40;

// ---- BMI270 FIFO (datasheet 4.7, registers 0x24-0x26 and 0x48-0x49) ----
//
// Polling the data registers loses a sample whenever the cooperative executor
// wakes the sensor task more than one ODR period late (a long UI raster), and
// the sensor's own clock drifts against our 20 ms ticker. The FIFO buffers
// every sample, so the stream is exactly the sensor ODR regardless of when the
// task gets to run.
const REG_FIFO_LENGTH_0: u8 = 0x24;
const REG_FIFO_DATA: u8 = 0x26;
const REG_FIFO_CONFIG_0: u8 = 0x48;
const REG_FIFO_CONFIG_1: u8 = 0x49;
/// Stream mode: overwrite the oldest frame when full.
const FIFO_CONFIG_0_STREAM: u8 = 0x00;
/// gyr_en | acc_en, headerless: every frame is gyro (6 B) then accel (6 B).
const FIFO_CONFIG_1_ACC_GYR_HEADERLESS: u8 = 0xC0;
const CMD_FIFO_FLUSH: u8 = 0xB0;
const FIFO_FRAME_LEN: usize = 12;
/// Most frames drained per call (320 ms at 50 Hz). A larger backlog drains on
/// the following ticks.
pub const FIFO_MAX_FRAMES: usize = 16;
/// The BMI270 FIFO is 2 KiB; anything larger is a bad length read.
const FIFO_CAPACITY: usize = 2048;

/// One accel/gyro sample. Each axis vector is present only when the matching
/// data-ready bit was set in the burst.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuSample {
    pub accel_raw: Option<[i16; 3]>,
    pub gyro_raw: Option<[i16; 3]>,
    pub accel_mg: Option<[i16; 3]>,
    pub gyro_dps: Option<[i16; 3]>,
}

/// One headerless FIFO frame: gyro XYZ then accel XYZ, little endian.
fn parse_fifo_frame(frame: &[u8]) -> ImuSample {
    let word = |index: usize| i16::from_le_bytes([frame[index], frame[index + 1]]);
    ImuSample {
        accel_raw: Some([word(6), word(8), word(10)]),
        gyro_raw: Some([word(0), word(2), word(4)]),
        gyro_dps: Some([
            scale_i16(word(0), 2000, 32768),
            scale_i16(word(2), 2000, 32768),
            scale_i16(word(4), 2000, 32768),
        ]),
        accel_mg: Some([
            scale_i16(word(6), 1000, 16384),
            scale_i16(word(8), 1000, 16384),
            scale_i16(word(10), 1000, 16384),
        ]),
    }
}

/// The BMI270 driver.
pub struct Bmi270 {
    addr: u8,
    fifo: bool,
}

impl Bmi270 {
    /// Probe the documented addresses and return the device that answers with
    /// chip ID `0x24`. `0x68` is tried first because the BSP names it;
    /// `0x69` is the official BSP address.
    pub async fn probe() -> Option<Self> {
        for addr in [0x68u8, 0x69u8] {
            let mut id = [0u8; 1];
            match shared_i2c::read_regs_async(addr, REG_CHIP_ID, &mut id).await {
                Some(Ok(())) if id[0] == BMI270_CHIP_ID => {
                    info!("BMI270 found at 0x{addr:02X} (chip id 0x{:02X})", id[0]);
                    return Some(Self { addr, fifo: false });
                }
                Some(Ok(())) => {
                    info!(
                        "BMI270 probe 0x{addr:02X}: unexpected chip id 0x{:02X}",
                        id[0]
                    );
                }
                Some(Err(error)) => info!("BMI270 probe 0x{addr:02X}: {error}"),
                None => break, // shared bus not installed
            }
        }
        warn!("BMI270 not found at 0x68 or 0x69");
        None
    }

    /// Load the Bosch configuration file, verify `INTERNAL_STATUS`, and enable
    /// accel + gyro at 50 Hz.
    ///
    /// The 30-byte chunks plus the register byte fit the BSP's 32-byte I2C
    /// write limit. Word addresses are `byte_offset / 2`.
    pub async fn configure(&mut self) -> Result<(), &'static str> {
        // Soft reset, then disable advanced power save before the burst upload.
        self.write_reg(REG_CMD, CMD_SOFT_RESET).await?;
        Timer::after(Duration::from_millis(2)).await;
        self.write_reg(REG_PWR_CONF, 0x00).await?;
        Timer::after(Duration::from_micros(450)).await;
        self.write_reg(REG_INIT_CTRL, 0x00).await?;

        for (index, chunk) in BMI270_CONFIG.chunks(30).enumerate() {
            let word = (index * 15) as u16;
            self.write_init_addr(word).await?;
            self.write_init_data(chunk).await?;
        }

        self.write_reg(REG_INIT_CTRL, 0x01).await?;
        Timer::after(Duration::from_millis(20)).await;

        let mut status = [0u8; 1];
        self.read_regs(REG_INTERNAL_STATUS, &mut status).await?;
        if status[0] & 0x0F != 0x01 {
            warn!(
                "BMI270 configuration load failed: INTERNAL_STATUS=0x{:02X}",
                status[0]
            );
            return Err("BMI270 config load status");
        }

        self.write_reg(REG_ACC_CONF, ACC_CONF_50HZ).await?;
        self.write_reg(REG_ACC_RANGE, ACC_RANGE_2G).await?;
        self.write_reg(REG_GYR_CONF, GYR_CONF_50HZ).await?;
        self.write_reg(REG_GYR_RANGE, GYR_RANGE_2000DPS).await?;
        self.write_reg(REG_PWR_CTRL, PWR_CTRL_ACC_GYR).await?;
        // Gyro power-up takes longer than the accelerometer; 100 ms covers the
        // datasheet start-up time before the first forced sample is requested.
        Timer::after(Duration::from_millis(100)).await;

        self.fifo = self.enable_fifo().await.is_ok();
        info!(
            "BMI270 ready @0x{:02X}: accel ±2 g 50 Hz, gyro ±2000 dps 50 Hz, {}",
            self.addr,
            if self.fifo { "FIFO" } else { "register poll" }
        );
        Ok(())
    }

    async fn enable_fifo(&mut self) -> Result<(), &'static str> {
        self.write_reg(REG_FIFO_CONFIG_0, FIFO_CONFIG_0_STREAM)
            .await?;
        self.write_reg(REG_FIFO_CONFIG_1, FIFO_CONFIG_1_ACC_GYR_HEADERLESS)
            .await?;
        let mut check = [0u8; 1];
        self.read_regs(REG_FIFO_CONFIG_1, &mut check).await?;
        if check[0] & 0xD0 != FIFO_CONFIG_1_ACC_GYR_HEADERLESS {
            return Err("BMI270 FIFO config did not stick");
        }
        self.write_reg(REG_CMD, CMD_FIFO_FLUSH).await
    }

    /// Drain up to [`FIFO_MAX_FRAMES`] buffered samples, oldest first.
    ///
    /// Falls back to one register burst when the FIFO is not enabled. Returns
    /// the number of samples written to `out`; `Ok(0)` means nothing new.
    pub async fn read_all(
        &mut self,
        out: &mut [ImuSample; FIFO_MAX_FRAMES],
    ) -> Result<usize, &'static str> {
        if !self.fifo {
            return match self.read().await? {
                Some(sample) => {
                    out[0] = sample;
                    Ok(1)
                }
                None => Ok(0),
            };
        }

        let mut length = [0u8; 2];
        self.read_regs(REG_FIFO_LENGTH_0, &mut length).await?;
        let bytes = usize::from(length[0]) | (usize::from(length[1] & 0x3F) << 8);
        if bytes > FIFO_CAPACITY {
            self.write_reg(REG_CMD, CMD_FIFO_FLUSH).await?;
            return Err("BMI270 FIFO length out of range");
        }
        let frames = (bytes / FIFO_FRAME_LEN).min(FIFO_MAX_FRAMES);
        if frames == 0 {
            return Ok(0);
        }

        let mut data = [0u8; FIFO_FRAME_LEN * FIFO_MAX_FRAMES];
        let data = &mut data[..frames * FIFO_FRAME_LEN];
        self.read_regs(REG_FIFO_DATA, data).await?;
        for (slot, frame) in out.iter_mut().zip(data.chunks_exact(FIFO_FRAME_LEN)) {
            *slot = parse_fifo_frame(frame);
        }
        Ok(frames)
    }

    /// Auxiliary measurements for the Lua snapshot. No status register is
    /// read from Lua; this runs solely in the owning sensor task.
    pub async fn read_aux(&mut self) -> Result<(i16, u32, u8), &'static str> {
        let mut temperature = [0; 2];
        let mut time = [0; 3];
        let mut status = [0; 1];
        self.read_regs(0x22, &mut temperature).await?;
        self.read_regs(0x18, &mut time).await?;
        self.read_regs(REG_STATUS, &mut status).await?;
        Ok((
            i16::from_le_bytes(temperature),
            u32::from_le_bytes([time[0], time[1], time[2], 0]),
            status[0],
        ))
    }

    /// Read status + accel + gyro in one burst.
    ///
    /// `Ok(None)` means the bus transaction succeeded but no data-ready bit was
    /// set; `Err` is a transport failure.
    pub async fn read(&mut self) -> Result<Option<ImuSample>, &'static str> {
        let mut burst = [0u8; IMU_BURST_LEN];
        self.read_regs(REG_STATUS, &mut burst).await?;

        let status = burst[0];
        let mut sample = ImuSample::default();

        if status & IMU_STATUS_ACC_READY != 0 {
            let x = i16::from_le_bytes([burst[9], burst[10]]);
            let y = i16::from_le_bytes([burst[11], burst[12]]);
            let z = i16::from_le_bytes([burst[13], burst[14]]);
            // ±2 g full scale: raw * 1000 mg / 16384.
            sample.accel_raw = Some([x, y, z]);
            sample.accel_mg = Some([
                scale_i16(x, 1000, 16384),
                scale_i16(y, 1000, 16384),
                scale_i16(z, 1000, 16384),
            ]);
        }

        if status & IMU_STATUS_GYR_READY != 0 {
            let x = i16::from_le_bytes([burst[15], burst[16]]);
            let y = i16::from_le_bytes([burst[17], burst[18]]);
            let z = i16::from_le_bytes([burst[19], burst[20]]);
            // ±2000 dps full scale: raw * 2000 dps / 32768.
            sample.gyro_raw = Some([x, y, z]);
            sample.gyro_dps = Some([
                scale_i16(x, 2000, 32768),
                scale_i16(y, 2000, 32768),
                scale_i16(z, 2000, 32768),
            ]);
        }

        if sample.accel_mg.is_none() && sample.gyro_dps.is_none() {
            Ok(None)
        } else {
            Ok(Some(sample))
        }
    }

    async fn write_reg(&self, reg: u8, value: u8) -> Result<(), &'static str> {
        match shared_i2c::write_reg_async(self.addr, reg, value).await {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("BMI270 I2C write"),
            None => Err("shared I2C not installed"),
        }
    }

    async fn read_regs(&self, reg: u8, data: &mut [u8]) -> Result<(), &'static str> {
        match shared_i2c::read_regs_async(self.addr, reg, data).await {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("BMI270 I2C read"),
            None => Err("shared I2C not installed"),
        }
    }

    async fn write_init_addr(&self, word: u16) -> Result<(), &'static str> {
        let payload = [(word & 0x0F) as u8, (word >> 4) as u8];
        let mut frame = [0u8; 3];
        frame[0] = REG_INIT_ADDR_0;
        frame[1..].copy_from_slice(&payload);
        let result = match shared_i2c::write_async(self.addr, &frame).await {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("BMI270 init address write"),
            None => Err("shared I2C not installed"),
        };
        // Let a waiting touch read acquire the shared bus.
        embassy_futures::yield_now().await;
        result
    }

    async fn write_init_data(&self, chunk: &[u8]) -> Result<(), &'static str> {
        let mut frame = [0u8; 32];
        frame[0] = REG_INIT_DATA;
        frame[1..1 + chunk.len()].copy_from_slice(chunk);
        let result = match shared_i2c::write_async(self.addr, &frame[..1 + chunk.len()]).await {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("BMI270 init data write"),
            None => Err("shared I2C not installed"),
        };
        embassy_futures::yield_now().await;
        result
    }
}
