//! Camera SCCB identity, descriptor validation and pixel conversion.
//! Independent of HAL and UI so bus fixtures can validate the wire addresses.

#[path = "tables.rs"]
mod tables;

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 480;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * 2;
pub const PREVIEW: usize = 320;
pub const PREVIEW_BYTES: usize = PREVIEW * PREVIEW * 3;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensor {
    Ov3640,
    Sc101iot,
}
impl Sensor {
    pub fn address(self) -> u8 {
        match self {
            Self::Ov3640 => 0x3C,
            Self::Sc101iot => 0x68,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Ov3640 => "OV3640",
            Self::Sc101iot => "SC101IOT",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Bus,
    UnknownSensor,
    Descriptor,
    WrongModule,
    ShortFrame,
}
pub trait Sccb {
    fn write(&mut self, address: u8, data: &[u8]) -> Result<(), Error>;
    fn read(&mut self, address: u8, prefix: &[u8]) -> Result<u8, Error>;
}
pub fn read_register(bus: &mut impl Sccb, sensor: Sensor, reg: u16) -> Result<u8, Error> {
    match sensor {
        Sensor::Ov3640 => bus.read(sensor.address(), &reg.to_be_bytes()),
        Sensor::Sc101iot => {
            bus.write(sensor.address(), &[0xF0, (reg >> 8) as u8])?;
            bus.read(sensor.address(), &[reg as u8])
        }
    }
}
pub fn write_register(
    bus: &mut impl Sccb,
    sensor: Sensor,
    reg: u16,
    value: u8,
) -> Result<(), Error> {
    match sensor {
        Sensor::Ov3640 => bus.write(sensor.address(), &[(reg >> 8) as u8, reg as u8, value]),
        Sensor::Sc101iot => {
            bus.write(sensor.address(), &[0xF0, (reg >> 8) as u8])?;
            bus.write(sensor.address(), &[reg as u8, value])
        }
    }
}
pub fn detect(bus: &mut impl Sccb, sensor: Sensor) -> Result<Sensor, Error> {
    let (hi, lo) = match sensor {
        Sensor::Ov3640 => (0x300A, 0x300B),
        Sensor::Sc101iot => (0x31F7, 0x31F8),
    };
    let id = u16::from_be_bytes([
        read_register(bus, sensor, hi)?,
        read_register(bus, sensor, lo)?,
    ]);
    match (sensor, id) {
        (Sensor::Ov3640, 0x364C | 0x3641) | (Sensor::Sc101iot, 0xDA4A) => Ok(sensor),
        _ => Err(Error::UnknownSensor),
    }
}
/// App visibility is the capture lease. Leaving cancels initialization, DMA and
/// conversion; only entry or explicit retry may start a failed session again.
#[derive(Default)]
pub struct Session {
    active: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub enum SessionAction {
    None,
    Start,
    Stop,
}
impl Session {
    pub fn update(&mut self, active: bool, retry: bool) -> SessionAction {
        let prior = core::mem::replace(&mut self.active, active);
        if !active && prior {
            SessionAction::Stop
        } else if active && (!prior || retry) {
            SessionAction::Start
        } else {
            SessionAction::None
        }
    }
}
/// Bounded register initialization shared by the firmware and bus fixtures.
/// An I/O failure never reports completion. OV reset receives a timed settle.
#[derive(Default)]
pub struct InitRegisters {
    index: usize,
    resume_us: u64,
}
impl InitRegisters {
    pub fn advance(
        &mut self,
        bus: &mut impl Sccb,
        sensor: Sensor,
        now: u64,
    ) -> Result<bool, Error> {
        if now < self.resume_us {
            return Ok(false);
        }
        let table = match sensor {
            Sensor::Ov3640 => tables::OV3640,
            Sensor::Sc101iot => tables::SC101IOT,
        };
        for _ in 0..4 {
            let Some(&(reg, value)) = table.get(self.index) else {
                return Ok(true);
            };
            match sensor {
                Sensor::Ov3640 => write_register(bus, sensor, reg, value)?,
                // Vendor SC table includes explicit F0 page writes; preserve it.
                Sensor::Sc101iot => bus.write(sensor.address(), &[reg as u8, value])?,
            }
            self.index += 1;
            if sensor == Sensor::Ov3640 && reg == 0x3012 && value == 0x80 {
                self.resume_us = now + 20_000;
                return Ok(false);
            }
        }
        Ok(self.index == table.len())
    }
}
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFF;
    for byte in data {
        crc ^= *byte as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}
pub fn camera_descriptor(raw: &[u8]) -> Result<(), Error> {
    if raw.len() != 0x86 || &raw[..3] != b"ESP" {
        return Err(Error::Descriptor);
    }
    for (start, end) in [(0, 0x34), (0x36, 0x3E), (0x40, 0x84)] {
        if crc16(&raw[start..end]) != u16::from_le_bytes([raw[end], raw[end + 1]]) {
            return Err(Error::Descriptor);
        }
    }
    if u16::from_le_bytes([raw[0x42], raw[0x43]]) > 64 {
        return Err(Error::Descriptor);
    }
    if raw[3] != 0x07 {
        return Err(Error::WrongModule);
    }
    Ok(())
}
pub const FRAME_TAIL_BYTES: usize = 64;
pub const CAPTURE_TIMEOUT_US: u64 = 1_000_000;
/// A complete VSYNC-delimited frame may include up to 64 pipeline-tail bytes.
/// The BSP frame stability patch rejects short frames but discards extra tail
/// samples; only the first FRAME_BYTES become pixels. Capacity alone is not EOF.
pub fn validate_frame(received: usize, first_eof: Option<usize>) -> Result<(), Error> {
    match first_eof {
        Some(eof)
            if (FRAME_BYTES..=FRAME_BYTES + FRAME_TAIL_BYTES).contains(&eof)
                && received >= eof
                && received <= FRAME_BYTES + FRAME_TAIL_BYTES =>
        {
            Ok(())
        }
        _ => Err(Error::ShortFrame),
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum CaptureAction {
    Convert,
    Discard,
    Incomplete,
    Timeout,
    DmaError,
}
/// Reject every partial frame. Allow at most three consecutive attempts to
/// synchronize at the first VSYNC; a stable stream resets this bounded budget.
pub fn capture_action(
    received: usize,
    eof: Option<usize>,
    dma_error: bool,
    elapsed: u64,
    failed_attempts: u8,
) -> CaptureAction {
    if dma_error {
        CaptureAction::DmaError
    } else if validate_frame(received, eof).is_ok() {
        CaptureAction::Convert
    } else if elapsed >= CAPTURE_TIMEOUT_US {
        CaptureAction::Timeout
    } else if failed_attempts < 2 {
        CaptureAction::Discard
    } else {
        CaptureAction::Incomplete
    }
}
/// Center-crop 480x480 and rotate counterclockwise, matching the official module
/// orientation. Output is independently owned RGB8; input DMA must be stopped.
pub fn source_xy(x: usize, y: usize) -> (usize, usize) {
    let px = (2 * x + 1) * HEIGHT / (2 * PREVIEW);
    let py = (2 * y + 1) * HEIGHT / (2 * PREVIEW);
    ((WIDTH - HEIGHT) / 2 + HEIGHT - 1 - py, px)
}
fn rgb(sensor: Sensor, source: &[u8], x: usize, y: usize) -> [u8; 3] {
    let i = (y * WIDTH + x) * 2;
    match sensor {
        Sensor::Ov3640 => {
            let p = u16::from_le_bytes([source[i], source[i + 1]]);
            let r = ((p >> 11) & 31) as u8;
            let g = ((p >> 5) & 63) as u8;
            let b = (p & 31) as u8;
            [
                (r << 3) | (r >> 2),
                (g << 2) | (g >> 4),
                (b << 3) | (b >> 2),
            ]
        }
        Sensor::Sc101iot => {
            let pair = (y * WIDTH + (x & !1)) * 2;
            let c = (source[pair + if x & 1 == 0 { 1 } else { 3 }] as i32 - 16).max(0);
            let d = source[pair] as i32 - 128;
            let e = source[pair + 2] as i32 - 128;
            let clip = |v: i32| ((v + 128) >> 8).clamp(0, 255) as u8;
            [
                clip(298 * c + 409 * e),
                clip(298 * c - 100 * d - 208 * e),
                clip(298 * c + 516 * d),
            ]
        }
    }
}
pub fn convert_rows(
    sensor: Sensor,
    source: &[u8],
    target: &mut [u8],
    start: usize,
    end: usize,
) -> Result<(), Error> {
    if source.len() < FRAME_BYTES || target.len() != PREVIEW_BYTES || start > end || end > PREVIEW {
        return Err(Error::ShortFrame);
    }
    for y in start..end {
        for x in 0..PREVIEW {
            let (sx, sy) = source_xy(x, y);
            let i = (y * PREVIEW + x) * 3;
            target[i..i + 3].copy_from_slice(&rgb(sensor, source, sx, sy));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Bus {
        sensor: Sensor,
        page: u8,
        fail_low: bool,
        wrong: bool,
    }
    impl Sccb for Bus {
        fn write(&mut self, a: u8, d: &[u8]) -> Result<(), Error> {
            assert_eq!(a, 0x68);
            assert_eq!(d[0], 0xF0);
            self.page = d[1];
            Ok(())
        }
        fn read(&mut self, a: u8, p: &[u8]) -> Result<u8, Error> {
            let (reg, hi, lo) = match self.sensor {
                Sensor::Ov3640 => {
                    assert_eq!(a, 0x3C);
                    assert_eq!(p.len(), 2);
                    (u16::from_be_bytes([p[0], p[1]]), 0x36, 0x4C)
                }
                Sensor::Sc101iot => {
                    assert_eq!(a, 0x68);
                    assert_eq!(p.len(), 1);
                    assert_eq!(self.page, 0x31);
                    ((self.page as u16) << 8 | p[0] as u16, 0xDA, 0x4A)
                }
            };
            match reg {
                0x300A | 0x31F7 => Ok(if self.wrong { 0 } else { hi }),
                0x300B | 0x31F8 => {
                    if self.fail_low {
                        Err(Error::Bus)
                    } else {
                        Ok(lo)
                    }
                }
                _ => panic!("unexpected ID register"),
            }
        }
    }
    #[test]
    fn official_id_bus_fixtures() {
        for sensor in [Sensor::Ov3640, Sensor::Sc101iot] {
            let mut b = Bus {
                sensor,
                page: 0,
                fail_low: false,
                wrong: false,
            };
            assert_eq!(detect(&mut b, sensor), Ok(sensor));
            b.fail_low = true;
            assert_eq!(detect(&mut b, sensor), Err(Error::Bus));
            b.fail_low = false;
            b.wrong = true;
            assert_eq!(detect(&mut b, sensor), Err(Error::UnknownSensor));
        }
    }
    #[test]
    fn rgb565_little_endian_colors() {
        let mut f = vec![0; FRAME_BYTES];
        for (p, want) in [
            (0xF800u16, [255, 0, 0]),
            (0x07E0, [0, 255, 0]),
            (0x001F, [0, 0, 255]),
        ] {
            f[..2].copy_from_slice(&p.to_le_bytes());
            assert_eq!(rgb(Sensor::Ov3640, &f, 0, 0), want);
        }
    }
    #[test]
    fn uyvy_fixed_colors() {
        let mut f = vec![0; FRAME_BYTES];
        for (pair, want) in [
            ([128, 16, 128, 235], ([0, 0, 0], [255, 255, 255])),
            ([90, 81, 240, 81], ([255, 0, 0], [255, 0, 0])),
        ] {
            f[..4].copy_from_slice(&pair);
            assert_eq!(rgb(Sensor::Sc101iot, &f, 0, 0), want.0);
            assert_eq!(rgb(Sensor::Sc101iot, &f, 1, 0), want.1);
        }
    }
    #[test]
    fn ccw_crop_direction() {
        assert_eq!(source_xy(0, 0), (559, 0));
        assert_eq!(source_xy(319, 0), (559, 479));
        assert_eq!(source_xy(0, 319), (80, 0));
        assert_eq!(source_xy(319, 319), (80, 479));
    }
    #[test]
    fn rejects_partial_and_non_eof_frames() {
        assert_eq!(
            validate_frame(FRAME_BYTES - 1, Some(FRAME_BYTES - 1)),
            Err(Error::ShortFrame)
        );
        assert_eq!(validate_frame(FRAME_BYTES, None), Err(Error::ShortFrame));
        assert_eq!(validate_frame(FRAME_BYTES + 64, Some(FRAME_BYTES)), Ok(()));
        let mut out = vec![0; PREVIEW_BYTES];
        assert_eq!(
            convert_rows(Sensor::Ov3640, &[0; 4], &mut out, 0, 320),
            Err(Error::ShortFrame)
        );
    }
    #[test]
    fn validates_all_descriptor_sections() {
        let mut d = [0; 0x86];
        d[..3].copy_from_slice(b"ESP");
        d[3] = 7;
        for (s, e) in [(0, 0x34), (0x36, 0x3E), (0x40, 0x84)] {
            let c = crc16(&d[s..e]);
            d[e..e + 2].copy_from_slice(&c.to_le_bytes());
        }
        assert_eq!(camera_descriptor(&d), Ok(()));
        for idx in [0x10, 0x38, 0x50] {
            let mut bad = d;
            bad[idx] ^= 1;
            assert_eq!(camera_descriptor(&bad), Err(Error::Descriptor));
        }
    }
    struct Writes {
        expected: Vec<(u8, Vec<u8>)>,
        fail_at: Option<usize>,
        calls: usize,
    }
    impl Sccb for Writes {
        fn write(&mut self, a: u8, d: &[u8]) -> Result<(), Error> {
            if self.fail_at == Some(self.calls) {
                return Err(Error::Bus);
            }
            if !self.expected.is_empty() {
                let (ea, ed) = self.expected.remove(0);
                assert_eq!(a, ea);
                assert_eq!(d, ed);
            }
            self.calls += 1;
            Ok(())
        }
        fn read(&mut self, _: u8, _: &[u8]) -> Result<u8, Error> {
            panic!("write fixture read")
        }
    }
    #[test]
    fn register_writes_use_sensor_wire_formats() {
        let mut b = Writes {
            expected: vec![
                (0x3C, vec![0x30, 0x86, 0x03]),
                (0x68, vec![0xF0, 0x31]),
                (0x68, vec![0x00, 0x01]),
            ],
            fail_at: None,
            calls: 0,
        };
        write_register(&mut b, Sensor::Ov3640, 0x3086, 3).unwrap();
        write_register(&mut b, Sensor::Sc101iot, 0x3100, 1).unwrap();
        assert!(b.expected.is_empty());
        b.fail_at = Some(b.calls);
        assert_eq!(
            write_register(&mut b, Sensor::Sc101iot, 0x3100, 1),
            Err(Error::Bus)
        );
        assert_eq!(
            write_register(&mut b, Sensor::Ov3640, 0x3086, 0),
            Err(Error::Bus)
        );
    }
    #[test]
    fn initialization_failure_and_reset_settle() {
        let mut b = Writes {
            expected: vec![(0x3C, vec![0x30, 0x12, 0x80])],
            fail_at: None,
            calls: 0,
        };
        let mut init = InitRegisters::default();
        assert_eq!(init.advance(&mut b, Sensor::Ov3640, 100), Ok(false));
        assert_eq!(init.advance(&mut b, Sensor::Ov3640, 20_099), Ok(false));
        assert_eq!(b.calls, 1);
        b.fail_at = Some(1);
        assert_eq!(
            init.advance(&mut b, Sensor::Ov3640, 20_100),
            Err(Error::Bus)
        );
        assert_eq!(b.calls, 1);
        b.fail_at = None;
        while !init.advance(&mut b, Sensor::Ov3640, 30_000).unwrap() {}
        assert_eq!(b.calls, 89);
        let mut init = InitRegisters::default();
        b.calls = 0;
        b.fail_at = Some(2);
        assert_eq!(init.advance(&mut b, Sensor::Sc101iot, 0), Err(Error::Bus));
        assert_eq!(b.calls, 2);
    }
    #[test]
    fn active_session_cancels_and_retry_restarts() {
        let mut s = Session::default();
        assert_eq!(s.update(false, true), SessionAction::None);
        assert_eq!(s.update(true, false), SessionAction::Start);
        // A timeout/init failure leaves the same lease; no automatic loop restart.
        assert_eq!(s.update(true, false), SessionAction::None);
        assert_eq!(s.update(true, true), SessionAction::Start);
        assert_eq!(s.update(false, false), SessionAction::Stop);
        assert_eq!(s.update(false, false), SessionAction::None);
        assert_eq!(s.update(true, false), SessionAction::Start);
    }
    #[test]
    fn descriptor_wrong_type_truncation_and_magic() {
        let mut d = [0; 0x86];
        d[..3].copy_from_slice(b"ESP");
        d[3] = 3;
        for (s, e) in [(0, 0x34), (0x36, 0x3E), (0x40, 0x84)] {
            let c = crc16(&d[s..e]);
            d[e..e + 2].copy_from_slice(&c.to_le_bytes());
        }
        assert_eq!(camera_descriptor(&d), Err(Error::WrongModule));
        assert_eq!(camera_descriptor(&d[..0x85]), Err(Error::Descriptor));
        d[0] = b'X';
        assert_eq!(camera_descriptor(&d), Err(Error::Descriptor));
    }
    #[test]
    fn capture_deadline_and_tail_boundaries() {
        for extra in [0, 2, 4, 64] {
            assert_eq!(
                capture_action(FRAME_BYTES + extra, Some(FRAME_BYTES + extra), false, 10, 0),
                CaptureAction::Convert
            );
        }
        assert_eq!(
            validate_frame(FRAME_BYTES, Some(FRAME_BYTES + 2)),
            Err(Error::ShortFrame)
        );
        assert_eq!(
            validate_frame(FRAME_BYTES + 65, Some(FRAME_BYTES + 65)),
            Err(Error::ShortFrame)
        );
        assert_eq!(
            capture_action(FRAME_BYTES, None, false, 10, 0),
            CaptureAction::Discard
        );
        assert_eq!(
            capture_action(FRAME_BYTES - 1, Some(FRAME_BYTES - 1), false, 10, 1),
            CaptureAction::Discard
        );
        assert_eq!(
            capture_action(FRAME_BYTES - 1, Some(FRAME_BYTES - 1), false, 10, 2),
            CaptureAction::Incomplete
        );
        assert_eq!(
            capture_action(0, None, false, CAPTURE_TIMEOUT_US, 0),
            CaptureAction::Timeout
        );
        assert_eq!(
            capture_action(FRAME_BYTES, Some(FRAME_BYTES), true, 10, 0),
            CaptureAction::DmaError
        );
    }
}
