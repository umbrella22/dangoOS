//! BMI270 at I2C0 @0x69. Bosch initialization/configuration is mandatory;
//! an initialization or transport failure never publishes a valid level.
mod protocol;

const ADDR: u8 = 0x69;
pub struct Bmi270;
impl Bmi270 {
    pub fn new() -> Option<Self> {
        let mut dev = Self;
        if let Err(error) = protocol::initialize(&mut dev) {
            log::warn!("BMI270 initialization failed: {error:?}");
            return None;
        }
        log::info!("BMI270 ready (Bosch config loaded, accel ±2g @100Hz)");
        Some(dev)
    }
    pub fn read_accel_mg(&mut self) -> Option<(f32, f32, f32)> {
        protocol::read_accel(self)
    }
}
impl protocol::SensorIo for Bmi270 {
    fn write(&mut self, reg: u8, data: &[u8]) -> Result<(), ()> {
        // The official BSP caps total I2C writes at 32 bytes.
        let mut buf = [0; 32];
        buf[0] = reg;
        buf[1..1 + data.len()].copy_from_slice(data);
        match super::shared_i2c::with_bus(|bus| bus.write(ADDR, &buf[..1 + data.len()])) {
            Some(Ok(())) => {
                esp_hal::delay::Delay::new().delay_micros(2);
                Ok(())
            }
            _ => Err(()),
        }
    }
    fn read(&mut self, reg: u8, data: &mut [u8]) -> Result<(), ()> {
        super::shared_i2c::with_bus(|bus| {
            bus.write(ADDR, &[reg]).is_ok() && bus.read(ADDR, data).is_ok()
        })
        .filter(|ok| *ok)
        .ok_or(())?;
        Ok(())
    }
    fn delay_us(&mut self, us: u32) {
        esp_hal::delay::Delay::new().delay_micros(us);
    }
}
