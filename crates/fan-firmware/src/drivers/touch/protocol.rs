// CST9220 report decoding, independent of the I2C transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchState {
    Up,
    Down { x: u16, y: u16 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportError {
    Header,
    Count,
    Coordinate,
}
pub fn register_command(reg: u16, params: &[u8], out: &mut [u8; 4]) -> usize {
    out[..2].copy_from_slice(&reg.to_be_bytes());
    out[2..2 + params.len()].copy_from_slice(params);
    2 + params.len()
}
pub fn decode(report: &[u8; 15], width: u16, height: u16) -> Result<TouchState, ReportError> {
    if report[0] == 0 {
        return Ok(TouchState::Up);
    }
    if report[0] == 0xAB || report[6] != 0xAB {
        return Err(ReportError::Header);
    }
    let count = (report[5] & 0x7F) as usize;
    if count > 2 {
        return Err(ReportError::Count);
    }
    for i in 0..count {
        let base = if i == 0 { 0 } else { i * 5 + 2 };
        if (report[base] & 0x0F) >> 1 == 3 {
            let x = ((report[base + 1] as u16) << 4) | (report[base + 3] >> 4) as u16;
            let y = ((report[base + 2] as u16) << 4) | (report[base + 3] & 0x0F) as u16;
            if x >= width || y >= height {
                return Err(ReportError::Coordinate);
            }
            return Ok(TouchState::Down { x, y });
        }
    }
    Ok(TouchState::Up)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(x: u16, y: u16) -> [u8; 15] {
        let mut r = [0; 15];
        r[0] = 6;
        r[1] = (x >> 4) as u8;
        r[2] = (y >> 4) as u8;
        r[3] = ((x & 15) << 4 | (y & 15)) as u8;
        r[5] = 1;
        r[6] = 0xAB;
        r
    }
    #[test]
    fn coordinates_and_release() {
        assert_eq!(
            decode(&frame(479, 479), 480, 480),
            Ok(TouchState::Down { x: 479, y: 479 })
        );
        assert_eq!(decode(&[0; 15], 480, 480), Ok(TouchState::Up));
    }
    #[test]
    fn malformed_frames_do_not_release() {
        let down = TouchState::Down { x: 20, y: 30 };
        let mut state = down;
        for mut r in [frame(480, 1), frame(1, 480), frame(1, 2)] {
            if r[1] == 0 && r[2] == 0 {
                r[6] = 0;
            }
            if let Ok(next) = decode(&r, 480, 480) {
                state = next;
            }
            assert_eq!(state, down);
        }
    }
    #[test]
    fn second_contact() {
        let mut r = frame(20, 30);
        r[0] = 4;
        r[5] = 2;
        r[7] = 6;
        r[8] = 10;
        r[9] = 12;
        assert_eq!(
            decode(&r, 480, 480),
            Ok(TouchState::Down { x: 160, y: 192 })
        );
    }
    #[test]
    fn two_byte_commands() {
        let mut b = [0; 4];
        let n = register_command(0xD101, &[], &mut b);
        assert_eq!(&b[..n], &[0xD1, 1]);
        let n = register_command(0xD000, &[0xAB], &mut b);
        assert_eq!(&b[..n], &[0xD0, 0, 0xAB]);
    }
}
