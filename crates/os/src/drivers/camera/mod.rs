//! Mosaico left-slot controls. Sensor SCCB and LCD_CAM are separate adapters.
pub mod dvp;
mod ov3640;
mod sc101iot;
pub mod slot;
use crate::drivers::shared_i2c;
use embassy_time::Timer;
use esp_hal::{
    gpio::{DriveMode, Flex, Level, Output, OutputConfig},
    peripherals::{GPIO34, GPIO48},
};

pub struct Controls {
    pwdn: Output<'static>,
    _flash: Output<'static>,
    pid: u16,
}
impl Controls {
    pub fn new(pwdn: GPIO48<'static>, flash: GPIO34<'static>) -> Self {
        Self {
            pwdn: Output::new(pwdn, Level::High, OutputConfig::default()),
            _flash: Output::new(
                flash,
                Level::High,
                OutputConfig::default().with_drive_mode(DriveMode::OpenDrain),
            ),
            pid: 0,
        }
    }
    pub async fn identify(&mut self, lease: &mut slot::Lease) -> Result<u16, &'static str> {
        self.pwdn.set_low();
        let mut reset = Flex::new(lease.reset.reborrow());
        reset.set_high();
        reset.apply_output_config(&OutputConfig::default());
        reset.set_output_enable(true);
        Timer::after_millis(20).await;
        reset.set_low();
        Timer::after_millis(10).await;
        reset.set_high();
        Timer::after_millis(40).await;
        // Sensors are detected at their own address/register pairs. A NACK
        // from the other supported model is an expected discovery result.
        for (addr, reg, expected) in [
            (0x3c, 0x300a, 0x364c),
            (0x3c, 0x300a, 0x3641),
            (0x68, 0x31f7, 0xda4a),
        ] {
            if addr == 0x68 {
                self.pwdn.set_high();
                Timer::after_millis(40).await;
            }
            let read = if addr == 0x68 {
                (
                    shared_i2c::camera_read_paged(addr, reg).await,
                    shared_i2c::camera_read_paged(addr, reg + 1).await,
                )
            } else {
                (
                    shared_i2c::camera_read_reg(addr, reg).await,
                    shared_i2c::camera_read_reg(addr, reg + 1).await,
                )
            };
            if let (Ok(hi), Ok(lo)) = read {
                let pid = u16::from_be_bytes([hi, lo]);
                log::info!("camera: SCCB addr=0x{addr:02x} pid=0x{pid:04x}");
                if pid == expected {
                    self.pid = pid;
                    return Ok(pid);
                }
            }
        }
        Err("sensor-not-found")
    }
    pub fn off(&mut self) {
        if self.pid == 0xda4a {
            self.pwdn.set_low();
        } else {
            self.pwdn.set_high();
        }
    }
    pub async fn configure(&mut self) -> Result<(), &'static str> {
        let table = match self.pid {
            0xda4a => sc101iot::VGA,
            0x3641 | 0x364c => ov3640::VGA,
            _ => return Err("unsupported-sensor"),
        };
        for &(reg, value) in table {
            if self.pid == 0xda4a && reg == 0xfe {
                Timer::after_millis(u64::from(value)).await;
                continue;
            }
            let result = if self.pid == 0xda4a {
                shared_i2c::camera_write_reg8(0x68, reg as u8, value).await
            } else {
                shared_i2c::camera_write_reg(0x3c, reg, value).await
            };
            result.map_err(|_| "sensor-config-write")?;
            if self.pid != 0xda4a && reg == 0x3012 && value == 0x80 {
                Timer::after_millis(10).await;
            }
        }
        if self.pid == 0xda4a {
            // Official Mosaico timing patch: 0 ns PCLK delay (bits 1:0).
            let value = shared_i2c::camera_read_paged(0x68, 0x3640)
                .await
                .map_err(|_| "sensor-pclk-read")?;
            shared_i2c::camera_write_reg8(0x68, 0xf0, 0x36)
                .await
                .map_err(|_| "sensor-pclk-page")?;
            shared_i2c::camera_write_reg8(0x68, 0x40, value & !3)
                .await
                .map_err(|_| "sensor-pclk-write")?;
            shared_i2c::camera_write_reg8(0x68, 0xf0, 0x31)
                .await
                .map_err(|_| "sensor-stream-page")?;
            let value = shared_i2c::camera_read_paged(0x68, 0x3100)
                .await
                .map_err(|_| "sensor-stream-read")?;
            shared_i2c::camera_write_reg8(0x68, 0x00, value | 1)
                .await
                .map_err(|_| "sensor-stream-write")?;
        }
        Timer::after_millis(100).await;
        Ok(())
    }
    pub fn release_reset(lease: &mut slot::Lease) {
        let mut reset = Flex::new(lease.reset.reborrow());
        reset.set_output_enable(false);
        reset.set_input_enable(false);
    }
}
