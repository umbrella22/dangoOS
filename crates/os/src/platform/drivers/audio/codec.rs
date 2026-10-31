//! ES8311 slave/analogue microphone, MCLK = 256 * Fs.
//! Register sequences derived from Espressif esp_codec_dev's ES8311 driver
//! (Apache-2.0), and the Mosaico v1.2 BSP (no DAC reference, active-high PA).
use crate::platform::buses::shared_i2c;
use embassy_time::Timer;
use esp_hal::gpio::{Level, Output, OutputConfig};

pub const ADDRESS: u8 = 0x19;
pub struct Codec {
    pa: Output<'static>,
}
impl Codec {
    pub fn new(pin: esp_hal::peripherals::GPIO45<'static>) -> Self {
        Self {
            pa: Output::new(pin, Level::Low, OutputConfig::default()),
        }
    }
    async fn write(reg: u8, value: u8) -> Result<(), &'static str> {
        shared_i2c::write_reg_async(ADDRESS, reg, value)
            .await
            .ok_or("audio I2C unavailable")?
            .map_err(|_| "ES8311 register write")
    }
    pub async fn read(reg: u8) -> Result<u8, &'static str> {
        let mut value = [0];
        shared_i2c::read_regs_async(ADDRESS, reg, &mut value)
            .await
            .ok_or("audio I2C unavailable")?
            .map_err(|_| "ES8311 register read")?;
        Ok(value[0])
    }
    async fn sequence(values: &[(u8, u8)]) -> Result<(), &'static str> {
        for &(reg, value) in values {
            Self::write(reg, value).await?;
        }
        Ok(())
    }
    pub async fn start(&mut self, rate: u32, volume: u8, muted: bool) -> Result<(), &'static str> {
        self.pa.set_low();
        if !matches!(rate, 16_000 | 32_000 | 44_100 | 48_000) {
            return Err("ES8311 sample rate");
        }
        // The second GPIO44 write is the vendor's first-write reliability rule.
        Self::sequence(&[
            (0x0d, 0xfa),
            (0x44, 0x08),
            (0x44, 0x08),
            (0x01, 0x30),
            (0x02, 0x00),
            (0x03, 0x10),
            (0x16, 0x24),
            (0x04, 0x10),
            (0x05, 0x00),
            (0x0b, 0x00),
            (0x0c, 0x00),
            (0x10, 0x1f),
            (0x11, 0x7f),
            (0x00, 0x80),
        ])
        .await?;
        Timer::after_millis(10).await;
        Self::sequence(&[
            (0x01, 0x3f),
            (0x06, 0x03),
            (0x13, 0x10),
            (0x1b, 0x0a),
            (0x1c, 0x6a),
            (0x44, 0x08),
            // 256-Fs coefficients: prediv=1, multiplier=1, ADC/DAC div=1.
            (0x02, 0x00),
            (0x03, 0x10),
            (0x04, if rate == 16_000 { 0x20 } else { 0x10 }),
            (0x05, 0x00),
            (0x06, 0x03),
            (0x07, 0x00),
            (0x08, 0xff),
            (0x09, 0x0c),
            (0x0a, 0x0c),
            (0x00, 0x80),
            (0x01, 0x3f),
            (0x17, 0xbf),
            (0x0e, 0x02),
            (0x12, 0x00),
            (0x14, 0x1a),
            (0x0d, 0x01),
            (0x15, 0x40),
            (0x37, 0x08),
            (0x45, 0x00),
            // Start with digital mute; the audio task enables PA after priming DMA.
            (0x31, 0x60),
            (0x16, 0x03),
        ])
        .await?;
        self.volume(volume).await?;
        self.mute(muted).await?;
        // Readback catches an ACKing bus/device that did not accept the format.
        if Self::read(0x09).await? & 0x1f != 0x0c || Self::read(0x0a).await? & 0x1f != 0x0c {
            return Err("ES8311 format readback");
        }
        Ok(())
    }
    pub async fn volume(&mut self, percent: u8) -> Result<(), &'static str> {
        if percent > 100 {
            return Err("audio volume range");
        }
        // -60 dB to 0 dB, with 0% implemented as digital mute by the service.
        Self::write(0x32, 71 + (u16::from(percent) * 120 / 100) as u8).await
    }
    pub async fn mute(&mut self, muted: bool) -> Result<(), &'static str> {
        let value = Self::read(0x31).await? & 0x9f;
        Self::write(0x31, value | if muted { 0x60 } else { 0 }).await
    }
    pub fn amplifier(&mut self, on: bool) {
        self.pa.set_level(if on { Level::High } else { Level::Low });
    }
    pub async fn stop(&mut self) -> Result<(), &'static str> {
        // Hardware cutoff precedes every fallible operation, including bus faults.
        self.pa.set_low();
        let mut result = Ok(());
        for &(reg, value) in &[
            (0x31, 0x60),
            (0x32, 0x00),
            (0x17, 0x00),
            (0x0e, 0xff),
            (0x12, 0x02),
            (0x14, 0x00),
            (0x0d, 0xfa),
            (0x15, 0x00),
            (0x02, 0x10),
            (0x00, 0x00),
            (0x00, 0x1f),
            (0x01, 0x30),
            (0x01, 0x00),
            (0x45, 0x00),
            (0x0d, 0xfc),
            (0x02, 0x00),
        ] {
            if let Err(error) = Self::write(reg, value).await {
                result = Err(error);
            }
        }
        result
    }
}
impl Drop for Codec {
    fn drop(&mut self) {
        self.pa.set_low();
    }
}
