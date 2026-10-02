//! BMI270 initialization and conversion, following Bosch BMI270 SensorAPI.
//! Vendor configuration and license are stored beside this module.
const CONFIG: &[u8; 8192] = include_bytes!("bmi270-config.bin");
pub trait SensorIo {
    fn write(&mut self, reg: u8, data: &[u8]) -> Result<(), ()>;
    fn read(&mut self, reg: u8, data: &mut [u8]) -> Result<(), ()>;
    fn delay_us(&mut self, us: u32);
}
#[derive(Debug, PartialEq, Eq)]
pub enum InitError {
    Bus,
    ChipId,
    ConfigStatus,
}
impl From<()> for InitError {
    fn from(_: ()) -> Self {
        Self::Bus
    }
}
pub fn initialize(io: &mut impl SensorIo) -> Result<(), InitError> {
    let mut value = [0];
    io.read(0x00, &mut value)?;
    if value[0] != 0x24 {
        return Err(InitError::ChipId);
    }
    io.write(0x7E, &[0xB6])?;
    io.delay_us(2_000);
    // APS is enabled after reset. Wait its 450us write interval before bursts.
    io.write(0x7C, &[0])?;
    io.delay_us(450);
    io.write(0x59, &[0])?;
    for (i, chunk) in CONFIG.chunks(30).enumerate() {
        let word = i * 30 / 2;
        io.write(0x5B, &[(word & 0x0F) as u8, (word >> 4) as u8])?;
        io.write(0x5E, chunk)?;
    }
    io.write(0x59, &[1])?;
    io.delay_us(20_000);
    io.read(0x21, &mut value)?;
    if value[0] & 0x0F != 1 {
        return Err(InitError::ConfigStatus);
    }
    io.write(0x40, &[0xA8, 0x00])?; // performance, avg4, 100Hz, ±2g
    io.write(0x7D, &[0x04])?; // accel only
    io.delay_us(20_000);
    Ok(())
}
pub fn read_accel(io: &mut impl SensorIo) -> Option<(f32, f32, f32)> {
    let mut ready = [0];
    io.read(0x03, &mut ready).ok()?;
    if ready[0] & 0x80 == 0 {
        return None;
    }
    let mut d = [0; 6];
    io.read(0x0C, &mut d).ok()?;
    let mg = |i| i16::from_le_bytes([d[i], d[i + 1]]) as f32 * (1000.0 / 16384.0);
    let xyz = (mg(0), mg(2), mg(4));
    // A zero vector cannot define a level; never label it a valid 0-degree sample.
    if xyz.0 == 0.0 && xyz.1 == 0.0 && xyz.2 == 0.0 {
        None
    } else {
        Some(xyz)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Bus {
        writes: Vec<(u8, Vec<u8>)>,
        delays: Vec<u32>,
        load_bad: bool,
        fail: bool,
        ready: bool,
    }
    impl SensorIo for Bus {
        fn write(&mut self, r: u8, d: &[u8]) -> Result<(), ()> {
            if self.fail {
                return Err(());
            }
            assert!(d.len() + 1 <= 32);
            self.writes.push((r, d.to_vec()));
            Ok(())
        }
        fn read(&mut self, r: u8, d: &mut [u8]) -> Result<(), ()> {
            if self.fail {
                return Err(());
            }
            match r {
                0 => d[0] = 0x24,
                0x21 => d[0] = if self.load_bad { 2 } else { 1 },
                3 => d[0] = if self.ready { 0x80 } else { 0 },
                0x0C => d.copy_from_slice(&[0, 0x40, 0, 0xC0, 0, 0x20]),
                _ => panic!("unexpected register"),
            };
            Ok(())
        }
        fn delay_us(&mut self, us: u32) {
            self.delays.push(us);
        }
    }
    #[test]
    fn complete_config_loaded_at_word_addresses() {
        let mut b = Bus::default();
        assert_eq!(initialize(&mut b), Ok(()));
        assert_eq!(
            &b.writes[..3],
            &[(0x7E, vec![0xB6]), (0x7C, vec![0]), (0x59, vec![0])]
        );
        let mut loaded = Vec::new();
        for (i, pair) in b.writes[3..3 + CONFIG.chunks(30).count() * 2]
            .chunks_exact(2)
            .enumerate()
        {
            assert_eq!(
                pair[0],
                (0x5B, vec![((i * 15) & 15) as u8, ((i * 15) >> 4) as u8])
            );
            assert_eq!(pair[1].0, 0x5E);
            loaded.extend_from_slice(&pair[1].1);
        }
        assert_eq!(loaded, CONFIG);
        assert!(b.writes.contains(&(0x59, vec![1])));
        assert!(b.writes.contains(&(0x40, vec![0xA8, 0])));
        assert!(b.delays.contains(&20000));
    }
    #[test]
    fn incomplete_config_is_not_ready() {
        let mut b = Bus {
            load_bad: true,
            ..Bus::default()
        };
        assert_eq!(initialize(&mut b), Err(InitError::ConfigStatus));
        assert!(!b.writes.iter().any(|(r, _)| *r == 0x7D));
    }
    #[test]
    fn failed_bus_aborts_init() {
        let mut b = Bus {
            fail: true,
            ..Bus::default()
        };
        assert_eq!(initialize(&mut b), Err(InitError::Bus));
    }
    #[test]
    fn ready_signed_16bit_acceleration() {
        let mut b = Bus::default();
        assert_eq!(read_accel(&mut b), None);
        b.ready = true;
        assert_eq!(read_accel(&mut b), Some((1000.0, -1000.0, 500.0)));
    }
}
