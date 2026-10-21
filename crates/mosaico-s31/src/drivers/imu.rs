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
//! answers with chip ID `0x32`. No address is assumed, and every candidate
//! that ACKs is logged with its raw chip-ID byte.
//!
//! Per BSP `onboard/sensors.c`, both BMM150s are on the **shared main I2C0**
//! (`BSP_I2C_PORT = I2C_NUM_0`, v1.2 SDA `GPIO56` / SCL `GPIO3`), together
//! with the BMI270 and CST9220. `sensor_task` also probes the right-slot
//! subboard I2C1 as a diagnostic fallback; the module EEPROM continues to use
//! that bus. A full 7-bit scan of both buses runs once at sensor startup.
//!
//! ## Rates
//!
//! Both sensors are configured for 50 Hz accelerometer/gyroscope and 20 Hz
//! magnetometer output. `sensor_task` polls at 50 Hz and publishes only new
//! samples.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use embassy_time::{Duration, Timer};
use esp_hal::i2c::master::Error as I2cError;
use log::{info, warn};

use crate::drivers::shared_i2c;

/// Unmodified Bosch `bmi270_config_file` (8192 bytes).
const BMI270_CONFIG: &[u8; 8192] = include_bytes!("../assets/bmi270-config.bin");

const BMI270_CHIP_ID: u8 = 0x24;
const BMM150_CHIP_ID: u8 = 0x32;

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

// ---- BMM150 registers (Bosch BMM150 datasheet / SensorAPI) ----
const BMM_CHIP_ID: u8 = 0x40;
const BMM_DATA_X_LSB: u8 = 0x42;
const BMM_DATA_READY_STATUS: u8 = 0x48;
const BMM_POWER_CONTROL: u8 = 0x4B;
const BMM_OP_MODE: u8 = 0x4C;
const BMM_REP_XY: u8 = 0x51;
const BMM_REP_Z: u8 = 0x52;
const BMM_DIG_X1: u8 = 0x5D;
const BMM_DIG_Z4_LSB: u8 = 0x62;
const BMM_DIG_Z2_LSB: u8 = 0x68;

const BMM_POWER_CNTRL_ENABLE: u8 = 0x01;
/// Bosch `BMM150_SET_SOFT_RESET` (`bmm150_defs.h`): write `0x4B` with
/// `0x80` (soft reset) | `0x02`. The BMI270 uses `0x7E = 0xB6`; that command
/// does not exist on the BMM150.
const BMM_SOFT_RESET: u8 = 0x82;
/// OP_MODE: normal mode (bits 1:0 = 0) with ODR bits 5:3 = 0x05 (20 Hz).
const BMM_OP_MODE_NORMAL_20HZ: u8 = 0x28;
/// Regular preset repetition counts.
const BMM_REP_XY_REGULAR: u8 = 0x04;
const BMM_REP_Z_REGULAR: u8 = 0x07;
const BMM_DRDY_STATUS_MSK: u8 = 0x01;

/// First conversion timeout from BSP `sensors.c`.
const BMM_DRDY_TIMEOUT_MS: u64 = 200;
const BMM_DRDY_POLL_MS: u64 = 5;

/// BMM150 axes overflow values.
const BMM_OVERFLOW_XY: i16 = -4096;
const BMM_OVERFLOW_Z: i16 = -16384;

/// Probe order for BMM150 addresses.
///
/// `0x11`/`0x12` are the official BSP 7-bit addresses; `0x10`/`0x13` are the
/// remaining Bosch-documented addresses (`BMM150_DEFAULT_I2C_ADDRESS` and the
/// CSB/SDO combinations); `0x22`/`0x24` are kept because RFC 0004 and the
/// factory YAML name them (they are the 8-bit form of `0x11`/`0x12`).
const BMM150_PROBE_ADDRS: [u8; 6] = [0x11, 0x12, 0x10, 0x13, 0x22, 0x24];

/// Inclusive 7-bit address range scanned once at startup.
const I2C_SCAN_FIRST: u8 = 0x08;
const I2C_SCAN_LAST: u8 = 0x77;

/// `I2C_SCAN_LAST - I2C_SCAN_FIRST + 1` bits, stored as four 32-bit words.
const SCAN_WORDS: usize = 4;
/// Two buses x four words. Bit `N` of word `N / 32` means address
/// `I2C_SCAN_FIRST + N` ACKed. Only bit 0..=111 are populated.
static SCAN_ACK: [[AtomicU32; SCAN_WORDS]; 2] = [
    [
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
    ],
    [
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
        AtomicU32::new(0),
    ],
];

/// Maximum candidates recorded by `Bmm150::probe_all`: six addresses on two
/// buses. Packed as `(bus << 16) | (addr << 8) | chip_id`.
const MAX_BMM_CANDIDATES: usize = 12;
static BMM_CANDIDATES: [AtomicU32; MAX_BMM_CANDIDATES] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];
static BMM_CANDIDATE_COUNT: AtomicU8 = AtomicU8::new(0);

impl MagBus {
    const fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Subboard => 1,
        }
    }
}

/// Persist one raw-scan ACK for the periodic `[scan]` summary.
fn record_ack(bus: MagBus, addr: u8) {
    if !(I2C_SCAN_FIRST..=I2C_SCAN_LAST).contains(&addr) {
        return;
    }
    let index = (addr - I2C_SCAN_FIRST) as usize;
    let word = index / 32;
    let bit = 1u32 << (index % 32);
    SCAN_ACK[bus.index()][word].fetch_or(bit, Ordering::Relaxed);
}

/// Persist one BMM150 probe candidate that ACKed its chip-ID read.
///
/// Layout: `bits 0..=7` initial raw ID, `bits 8..=15` address, `bit 16` bus,
/// `bits 24..=31` final ID after the power/reset/split/100 kHz ladder
/// (`0xFF` until the ladder finishes).
fn record_bmm_candidate(bus: MagBus, addr: u8, chip_id: u8) {
    let index = BMM_CANDIDATE_COUNT.load(Ordering::Relaxed) as usize;
    if index >= MAX_BMM_CANDIDATES {
        return;
    }
    let packed =
        (0xFFu32 << 24) | ((bus.index() as u32) << 16) | ((addr as u32) << 8) | chip_id as u32;
    BMM_CANDIDATES[index].store(packed, Ordering::Relaxed);
    BMM_CANDIDATE_COUNT.store((index + 1) as u8, Ordering::Release);
}

/// Update the final chip ID observed for a candidate after the probe ladder.
fn finish_bmm_candidate(bus: MagBus, addr: u8, final_id: u8) {
    let count = BMM_CANDIDATE_COUNT.load(Ordering::Acquire) as usize;
    let mut index = 0;
    while index < count.min(MAX_BMM_CANDIDATES) {
        let packed = BMM_CANDIDATES[index].load(Ordering::Relaxed);
        let entry_bus = ((packed >> 16) & 0x01) as usize;
        let entry_addr = ((packed >> 8) & 0xFF) as u8;
        if entry_bus == bus.index() && entry_addr == addr {
            let updated = (packed & 0x00FF_FFFF) | ((final_id as u32) << 24);
            BMM_CANDIDATES[index].store(updated, Ordering::Release);
            return;
        }
        index += 1;
    }
}

/// Compact `core::fmt::Write` sink for one `[scan] bmm=...` line.
struct ScanText {
    bytes: [u8; 140],
    len: usize,
}

impl ScanText {
    const fn new() -> Self {
        Self {
            bytes: [0; 140],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

impl core::fmt::Write for ScanText {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = self.bytes.len().saturating_sub(self.len);
        let count = core::cmp::min(available, text.len());
        self.bytes[self.len..self.len + count].copy_from_slice(&text.as_bytes()[..count]);
        self.len += count;
        Ok(())
    }
}

/// Write the persistent scan map and BMM candidates.
///
/// Called once after startup probing and then every 10 s by `sensor_task`, so
/// the result is visible on a USB CDC connection that attaches after boot.
/// The one-time detailed scan/probe lines are kept.
pub fn log_scan_summary() {
    let mut ack0 = [0u32; SCAN_WORDS];
    let mut ack1 = [0u32; SCAN_WORDS];
    for (index, word) in ack0.iter_mut().enumerate() {
        *word = SCAN_ACK[0][index].load(Ordering::Relaxed);
    }
    for (index, word) in ack1.iter_mut().enumerate() {
        *word = SCAN_ACK[1][index].load(Ordering::Relaxed);
    }
    // Words are printed MSB-first: the leftmost hex digit covers 0x68..=0x77,
    // the rightmost covers 0x08..=0x27.
    info!(
        "[scan] ack0={:08X}{:08X}{:08X}{:08X} ack1={:08X}{:08X}{:08X}{:08X}",
        ack0[3], ack0[2], ack0[1], ack0[0], ack1[3], ack1[2], ack1[1], ack1[0]
    );

    let count = BMM_CANDIDATE_COUNT.load(Ordering::Acquire) as usize;
    if count == 0 {
        info!("[scan] bmm=none (no ACK at 0x11/0x12/0x10/0x13/0x22/0x24 on I2C0/I2C1)");
        return;
    }

    let mut line = ScanText::new();
    let _ = write!(line, "bmm=");
    let mut any_valid = false;
    let mut index = 0;
    while index < count.min(MAX_BMM_CANDIDATES) {
        let packed = BMM_CANDIDATES[index].load(Ordering::Relaxed);
        let bus = (packed >> 16) & 0x01;
        let addr = ((packed >> 8) & 0xFF) as u8;
        let initial = (packed & 0xFF) as u8;
        let final_id = ((packed >> 24) & 0xFF) as u8;
        if initial == BMM150_CHIP_ID || final_id == BMM150_CHIP_ID {
            any_valid = true;
        }
        if index > 0 {
            let _ = line.write_str(",");
        }
        if final_id != 0xFF && final_id != initial {
            let _ = write!(line, "{bus}:{addr:02X}:{initial:02X}>{final_id:02X}");
        } else {
            let _ = write!(line, "{bus}:{addr:02X}:{initial:02X}");
        }
        index += 1;
    }
    if !any_valid {
        let _ = line.write_str(" (no 0x32)");
    }
    info!("[scan] {}", line.as_str());
}

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
enum MagReadMode {
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
    const fn khz(self) -> u32 {
        match self {
            Self::RepeatedStart | Self::Split => 400,
            Self::RepeatedStart100k | Self::Split100k => 100,
        }
    }
}

/// One address probe: a one-byte read ACKs the device without writing any
/// register. Used by the startup scan.
async fn bus_probe(bus: MagBus, addr: u8) -> Option<Result<(), I2cError>> {
    let mut byte = [0u8; 1];
    match bus {
        MagBus::Main => shared_i2c::read_async(addr, &mut byte).await,
        MagBus::Subboard => shared_i2c::subboard_read_async(addr, &mut byte).await,
    }
}

async fn bus_read_regs(
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

async fn bus_write_reg(bus: MagBus, addr: u8, reg: u8, value: u8) -> Option<Result<(), I2cError>> {
    match bus {
        MagBus::Main => shared_i2c::write_reg_async(addr, reg, value).await,
        MagBus::Subboard => shared_i2c::subboard_write_reg_async(addr, reg, value).await,
    }
}

/// Scan both I2C0 and I2C1 for every 7-bit address that ACKs.
///
/// Runs once at sensor-task startup. The output is the definitive bus map:
/// a BMM150 responds at one of `0x10..=0x13` with chip ID `0x32`. Addresses
/// are printed even when the device is not a BMM150 so the log shows whether
/// `0x11`/`0x12` are populated at all.
pub async fn scan_buses() {
    for bus_words in &SCAN_ACK {
        for word in bus_words {
            word.store(0, Ordering::Relaxed);
        }
    }

    for bus in [MagBus::Main, MagBus::Subboard] {
        let mut acked = 0u32;
        let mut addr = I2C_SCAN_FIRST;
        while addr <= I2C_SCAN_LAST {
            match bus_probe(bus, addr).await {
                Some(Ok(())) => {
                    acked = acked.saturating_add(1);
                    record_ack(bus, addr);
                    info!("i2c scan: {} 0x{:02X} ACK", bus.name(), addr);
                }
                Some(Err(_)) => {}
                None => {
                    warn!("i2c scan: {} bus is not installed", bus.name());
                    break;
                }
            }
            addr = addr.saturating_add(1);
        }
        info!(
            "i2c scan: {} complete, {} address(es) ACKed (0x{:02X}..=0x{:02X})",
            bus.name(),
            acked,
            I2C_SCAN_FIRST,
            I2C_SCAN_LAST
        );
    }
}

/// One accel/gyro sample. Each axis vector is present only when the matching
/// data-ready bit was set in the burst.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuSample {
    pub accel_mg: Option<[i16; 3]>,
    pub gyro_dps: Option<[i16; 3]>,
}

/// One headerless FIFO frame: gyro XYZ then accel XYZ, little endian.
fn parse_fifo_frame(frame: &[u8]) -> ImuSample {
    let word = |index: usize| i16::from_le_bytes([frame[index], frame[index + 1]]);
    ImuSample {
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

// ---- BMM150 ----

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
    pub x_ut: f32,
    pub y_ut: f32,
    pub z_ut: f32,
    /// Compensated vector magnitude in microtesla.
    pub magnitude_ut: f32,
    /// Raw 13/15-bit axes, kept for H1.2 calibration logs.
    pub raw: [i16; 3],
}

/// Result of the startup probe ladder for one address.
struct ProbeResult {
    chip_id: u8,
    read_mode: MagReadMode,
}

/// Read one register with a specific transfer form. Only the main I2C0 has
/// alternate-form helpers; the subboard bus always uses repeated start.
async fn read_reg_diag(bus: MagBus, addr: u8, reg: u8, mode: MagReadMode) -> Option<u8> {
    let mut data = [0u8; 1];
    let result = match (bus, mode) {
        (MagBus::Main, MagReadMode::RepeatedStart) => {
            shared_i2c::read_regs_async(addr, reg, &mut data).await
        }
        (MagBus::Main, MagReadMode::Split) => {
            shared_i2c::read_regs_split_async(addr, reg, &mut data).await
        }
        (MagBus::Main, MagReadMode::RepeatedStart100k) => {
            shared_i2c::read_regs_at_khz_async(addr, reg, &mut data, mode.khz()).await
        }
        (MagBus::Main, MagReadMode::Split100k) => {
            shared_i2c::read_regs_split_at_khz_async(addr, reg, &mut data, mode.khz()).await
        }
        (MagBus::Subboard, _) => shared_i2c::subboard_read_regs_async(addr, reg, &mut data).await,
    };
    match result {
        Some(Ok(())) => Some(data[0]),
        _ => None,
    }
}

/// Log the raw identity/power registers of a BMM150 candidate.
///
/// `0x00`/`0x01` are reserved on the BMM150 but are read because an all-zero
/// answer across unrelated registers is the evidence that the data phase is
/// not reaching the register file (or VDD is absent while VDDIO ACKs).
async fn log_bmm_regs(bus: MagBus, addr: u8, tag: &str) {
    let regs = [0x00u8, 0x01, BMM_CHIP_ID, BMM_POWER_CONTROL, BMM_OP_MODE];
    let mut values = [0xFFu8; 5];
    let mut index = 0;
    while index < regs.len() {
        if let Some(value) = read_reg_diag(bus, addr, regs[index], MagReadMode::RepeatedStart).await
        {
            values[index] = value;
        }
        index += 1;
    }
    info!(
        "BMM150 diag {} 0x{:02X} {tag}: 00={:02X} 01={:02X} 40={:02X} 4B={:02X} 4C={:02X}",
        bus.name(),
        addr,
        values[0],
        values[1],
        values[2],
        values[3],
        values[4]
    );
}

/// Bosch `bmm150_init` step: read `0x4B`, set the power-control bit, wait
/// 3 ms, then read the chip ID. Returns `(old_0x4B, chip_id)`.
async fn bmm_power_up(bus: MagBus, addr: u8) -> (Option<u8>, Option<u8>) {
    let mut power = [0u8; 1];
    let old = match bus_read_regs(bus, addr, BMM_POWER_CONTROL, &mut power).await {
        Some(Ok(())) => Some(power[0]),
        _ => None,
    };
    // Bosch reads 0x4B and sets bit 0. If that read failed, write the enable
    // bit directly rather than skipping the power-up.
    let new = old
        .map(|value| value | BMM_POWER_CNTRL_ENABLE)
        .unwrap_or(BMM_POWER_CNTRL_ENABLE);
    if !matches!(
        bus_write_reg(bus, addr, BMM_POWER_CONTROL, new).await,
        Some(Ok(()))
    ) {
        return (old, None);
    }
    // Bosch BMM150_START_UP_TIME = 3000 us.
    Timer::after(Duration::from_micros(3_000)).await;
    let id = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart).await;
    (old, id)
}

/// Bosch `bmm150_soft_reset`: `0x4B |= 0x82`, wait 1 ms, then power up again.
/// Returns the chip ID after the following 3 ms power-up delay.
async fn bmm_soft_reset_and_power_up(bus: MagBus, addr: u8) -> Option<u8> {
    let mut power = [0u8; 1];
    if !matches!(
        bus_read_regs(bus, addr, BMM_POWER_CONTROL, &mut power).await,
        Some(Ok(()))
    ) {
        return None;
    }
    if !matches!(
        bus_write_reg(bus, addr, BMM_POWER_CONTROL, power[0] | BMM_SOFT_RESET).await,
        Some(Ok(()))
    ) {
        return None;
    }
    // Bosch BMM150_DELAY_SOFT_RESET = 1000 us.
    Timer::after(Duration::from_micros(1_000)).await;
    let (_, id) = bmm_power_up(bus, addr).await;
    id
}

/// Power-up variant that also moves the register write to 100 kHz, for the
/// case where the 400 kHz write phase is the broken part of the transfer.
async fn bmm_power_up_100k(bus: MagBus, addr: u8) -> Option<u8> {
    if bus != MagBus::Main {
        return None;
    }
    let mut power = [0u8; 1];
    if !matches!(
        shared_i2c::read_regs_at_khz_async(addr, BMM_POWER_CONTROL, &mut power, 100).await,
        Some(Ok(()))
    ) {
        return None;
    }
    if !matches!(
        shared_i2c::write_reg_at_khz_async(
            addr,
            BMM_POWER_CONTROL,
            power[0] | BMM_POWER_CNTRL_ENABLE,
            100
        )
        .await,
        Some(Ok(()))
    ) {
        return None;
    }
    Timer::after(Duration::from_micros(3_000)).await;
    read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart100k).await
}

/// Startup probe ladder for one address.
///
/// Order: plain repeated-start read, split STOP/STOP read, Bosch power-up,
/// Bosch soft reset + power-up, 100 kHz repeated start, 100 kHz split. The
/// first form that yields `0x32` is kept for normal operation. Logs each step
/// and persists the final raw ID in the `[scan] bmm=` summary.
async fn probe_candidate(bus: MagBus, addr: u8) -> Option<ProbeResult> {
    let initial = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart).await?;
    record_bmm_candidate(bus, addr, initial);
    info!(
        "BMM150 probe {} 0x{:02X}: repeated-start 0x40=0x{:02X}",
        bus.name(),
        addr,
        initial
    );
    if initial == BMM150_CHIP_ID {
        finish_bmm_candidate(bus, addr, initial);
        return Some(ProbeResult {
            chip_id: initial,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    log_bmm_regs(bus, addr, "raw").await;

    // Write-based diagnostics are only safe on the official BMM150 addresses.
    if !matches!(addr, 0x11 | 0x12) {
        finish_bmm_candidate(bus, addr, initial);
        return Some(ProbeResult {
            chip_id: initial,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    let split = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::Split).await;
    info!(
        "BMM150 diag {} 0x{:02X}: split write/STOP/read 0x40={}",
        bus.name(),
        addr,
        optional_hex(split)
    );
    if split == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split,
        });
    }

    let (old_power, power_id) = bmm_power_up(bus, addr).await;
    info!(
        "BMM150 init {} 0x{:02X}: power 0x4B old={} -> |0x01, 3 ms, 0x40={}",
        bus.name(),
        addr,
        optional_hex(old_power),
        optional_hex(power_id)
    );
    if power_id == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    let split_powered = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::Split).await;
    info!(
        "BMM150 diag {} 0x{:02X}: powered split write/STOP/read 0x40={}",
        bus.name(),
        addr,
        optional_hex(split_powered)
    );
    if split_powered == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split,
        });
    }

    log_bmm_regs(bus, addr, "powered").await;

    let reset_id = bmm_soft_reset_and_power_up(bus, addr).await;
    info!(
        "BMM150 init {} 0x{:02X}: soft reset 0x4B|=0x82, 1 ms, power-up, 0x40={}",
        bus.name(),
        addr,
        optional_hex(reset_id)
    );
    if reset_id == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    let split_reset = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::Split).await;
    info!(
        "BMM150 diag {} 0x{:02X}: reset split write/STOP/read 0x40={}",
        bus.name(),
        addr,
        optional_hex(split_reset)
    );
    if split_reset == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split,
        });
    }

    let power_100k = bmm_power_up_100k(bus, addr).await;
    info!(
        "BMM150 init {} 0x{:02X}: 100 kHz power 0x4B|0x01, 3 ms, 0x40={}",
        bus.name(),
        addr,
        optional_hex(power_100k)
    );
    if power_100k == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart100k,
        });
    }

    let id_100k = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart100k).await;
    info!(
        "BMM150 diag {} 0x{:02X}: 100 kHz repeated-start 0x40={}",
        bus.name(),
        addr,
        optional_hex(id_100k)
    );
    if id_100k == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart100k,
        });
    }

    let split_100k = read_reg_diag(bus, addr, BMM_CHIP_ID, MagReadMode::Split100k).await;
    info!(
        "BMM150 diag {} 0x{:02X}: 100 kHz split 0x40={}",
        bus.name(),
        addr,
        optional_hex(split_100k)
    );
    if split_100k == Some(BMM150_CHIP_ID) {
        finish_bmm_candidate(bus, addr, BMM150_CHIP_ID);
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split100k,
        });
    }

    let final_id = split_100k
        .or(id_100k)
        .or(power_100k)
        .or(split_reset)
        .or(reset_id)
        .or(split_powered)
        .or(power_id)
        .or(split)
        .unwrap_or(initial);
    finish_bmm_candidate(bus, addr, final_id);
    log_unexpected_device(bus, addr).await;
    Some(ProbeResult {
        chip_id: final_id,
        read_mode: MagReadMode::RepeatedStart,
    })
}

fn optional_hex(value: Option<u8>) -> HexOpt {
    HexOpt(value)
}

/// Formats an optional register byte as `0xNN` or `ERR`.
struct HexOpt(Option<u8>);

impl core::fmt::Display for HexOpt {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(value) => write!(formatter, "0x{value:02X}"),
            None => formatter.write_str("ERR"),
        }
    }
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
    /// as a diagnostic fallback and is used only when it answers with a valid
    /// BMM150 chip ID.
    ///
    /// The probe follows Bosch `bmm150_init`: power control `0x4B |= 0x01`,
    /// wait 3 ms, then read chip ID `0x40`. The plain read is tried first and
    /// the remaining diagnostic ladder (split STOP/STOP transfer, soft reset
    /// `0x4B |= 0x82`, 100 kHz) runs only if the ID is still not `0x32`.
    pub async fn probe_all() -> [Option<Self>; 2] {
        let mut found: [Option<Self>; 2] = [None, None];
        let mut slots_full = false;

        for candidate in &BMM_CANDIDATES {
            candidate.store(0, Ordering::Relaxed);
        }
        BMM_CANDIDATE_COUNT.store(0, Ordering::Release);

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
            warn!(
                "BMM150: no chip id 0x32 on I2C0 or I2C1 at 0x11/0x12/0x10/0x13/0x22/0x24; \
                 see the [scan] bmm= summary for the final raw IDs"
            );
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

        Ok(Some(MagSample {
            x_ut,
            y_ut,
            z_ut,
            magnitude_ut,
            raw: [raw_x, raw_y, raw_z],
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

/// Capture the identity/state registers of an address that ACKed but did not
/// report the BMM150 chip ID. Read-only; no configuration is written.
async fn log_unexpected_device(bus: MagBus, addr: u8) {
    let mut power = [0xFFu8; 1];
    let mut op_mode = [0xFFu8; 1];
    let mut status = [0xFFu8; 1];
    let power_ok = matches!(
        bus_read_regs(bus, addr, BMM_POWER_CONTROL, &mut power).await,
        Some(Ok(()))
    );
    let op_ok = matches!(
        bus_read_regs(bus, addr, BMM_OP_MODE, &mut op_mode).await,
        Some(Ok(()))
    );
    let status_ok = matches!(
        bus_read_regs(bus, addr, BMM_DATA_READY_STATUS, &mut status).await,
        Some(Ok(()))
    );
    info!(
        "BMM150 probe {} 0x{:02X}: not chip id 0x32; 0x4B={} 0x4C={} 0x48={} (ok={}/{}/{})",
        bus.name(),
        addr,
        power[0],
        op_mode[0],
        status[0],
        power_ok as u8,
        op_ok as u8,
        status_ok as u8
    );
}

fn scale_i16(raw: i16, full_scale: i32, divisor: i32) -> i16 {
    ((raw as i32 * full_scale) / divisor) as i16
}

/// `f32::sqrt` is not available in `core`; use a fixed-point integer square
/// root on the squared magnitude.
fn sqrt_f32(value: f32) -> f32 {
    if value <= 0.0 {
        return 0.0;
    }
    // sqrt(value * 1e6) / 1e3 == sqrt(value); keep three fractional digits.
    isqrt((value * 1_000_000.0) as u64) as f32 / 1000.0
}

fn isqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}
