// CST9220 report decoding, independent of the I2C transport.
pub const REPORT_ACK: u8 = 0xAB;

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
/// An ACK left in D000 is not a fresh touch sample. Preserve the previous
/// pointer state until a contact or release report arrives.
pub fn decode(
    report: &[u8; 15],
    width: u16,
    height: u16,
) -> Result<Option<TouchState>, ReportError> {
    if report[0] == REPORT_ACK {
        return Ok(None);
    }
    if report[0] == 0 {
        return Ok(Some(TouchState::Up));
    }
    if report[6] != REPORT_ACK {
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
            return Ok(Some(TouchState::Down { x, y }));
        }
    }
    Ok(Some(TouchState::Up))
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
        r[6] = REPORT_ACK;
        r
    }
    #[test]
    fn coordinates_and_release() {
        assert_eq!(
            decode(&frame(479, 479), 480, 480),
            Ok(Some(TouchState::Down { x: 479, y: 479 }))
        );
        assert_eq!(decode(&[0; 15], 480, 480), Ok(Some(TouchState::Up)));
    }
    #[test]
    fn acknowledged_report_has_no_new_sample() {
        // The remaining bytes can still contain the previous report. They
        // must not turn this ACK into a new contact, release or parse error.
        for mut report in [[0; 15], [0xFF; 15], frame(20, 30)] {
            report[0] = REPORT_ACK;
            assert_eq!(decode(&report, 480, 480), Ok(None));
        }
    }
    #[test]
    fn idle_polls_do_not_split_a_drag_or_hide_the_release() {
        let mut idle = frame(20, 30);
        idle[0] = REPORT_ACK;
        let mut release = frame(40, 50);
        release[0] = 4; // The controller retains the count and coordinates.
        let reports = [frame(20, 30), idle, frame(40, 50), idle, release, idle];
        let samples: Vec<_> = reports
            .iter()
            .filter_map(|report| decode(report, 480, 480).unwrap())
            .collect();
        assert_eq!(
            samples,
            [
                TouchState::Down { x: 20, y: 30 },
                TouchState::Down { x: 40, y: 50 },
                TouchState::Up,
            ]
        );
    }
    #[test]
    fn fresh_reports_still_require_valid_header_count_and_coordinates() {
        let mut header = frame(20, 30);
        header[6] = 0;
        assert_eq!(decode(&header, 480, 480), Err(ReportError::Header));
        let mut count = frame(20, 30);
        count[5] = 3;
        assert_eq!(decode(&count, 480, 480), Err(ReportError::Count));
        assert_eq!(
            decode(&frame(480, 30), 480, 480),
            Err(ReportError::Coordinate)
        );
        assert_eq!(
            decode(&frame(20, 480), 480, 480),
            Err(ReportError::Coordinate)
        );
    }
    #[test]
    fn malformed_frames_do_not_release() {
        let down = TouchState::Down { x: 20, y: 30 };
        let mut state = down;
        for mut r in [frame(480, 1), frame(1, 480), frame(1, 2)] {
            if r[1] == 0 && r[2] == 0 {
                r[6] = 0;
            }
            if let Ok(Some(next)) = decode(&r, 480, 480) {
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
            Ok(Some(TouchState::Down { x: 160, y: 192 }))
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
