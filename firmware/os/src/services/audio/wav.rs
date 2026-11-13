//! RIFF/WAVE PCM boundaries. Unknown chunks retain their RIFF padding semantics.
use crate::services::audio::Format;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wave {
    pub format: Format,
    pub offset: usize,
    pub bytes: usize,
}
fn u16le(data: &[u8]) -> u16 {
    u16::from_le_bytes([data[0], data[1]])
}
fn u32le(data: &[u8]) -> u32 {
    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
}
pub fn parse(data: &[u8]) -> Result<Wave, &'static str> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file");
    }
    let end = usize::try_from(u32le(&data[4..8]))
        .ok()
        .and_then(|n| n.checked_add(8))
        .ok_or("WAVE length overflow")?;
    if end > data.len() || end < 12 {
        return Err("truncated RIFF");
    }
    let mut format = None;
    let mut payload = None;
    let mut at = 12usize;
    while at < end {
        if end - at < 8 {
            return Err("truncated WAVE chunk");
        }
        let size = u32le(&data[at + 4..at + 8]) as usize;
        let start = at + 8;
        let next = start.checked_add(size).ok_or("WAVE length overflow")?;
        if next > end {
            return Err("truncated WAVE payload");
        }
        match &data[at..at + 4] {
            b"fmt " => {
                if format.is_some() || size < 16 {
                    return Err("invalid WAVE format chunk");
                }
                let f = &data[start..next];
                if u16le(f) != 1 || u16le(&f[14..]) != 16 {
                    return Err("WAVE requires signed 16-bit PCM");
                }
                let channels = u16le(&f[2..]);
                let fmt = Format::new(
                    u32le(&f[4..]),
                    u8::try_from(channels).map_err(|_| "WAVE channels")?,
                )?;
                if usize::from(u16le(&f[12..])) != fmt.frame_bytes()
                    || u32le(&f[8..]) != fmt.rate * fmt.frame_bytes() as u32
                {
                    return Err("inconsistent WAVE frame layout");
                }
                format = Some(fmt);
            }
            b"data" => {
                if payload.replace((start, size)).is_some() {
                    return Err("multiple WAVE data chunks");
                }
            }
            _ => (),
        }
        at = next.checked_add(size & 1).ok_or("WAVE padding overflow")?;
        if at > end {
            return Err("missing RIFF pad byte");
        }
    }
    let format = format.ok_or("missing WAVE format")?;
    let (offset, bytes) = payload.ok_or("missing WAVE data")?;
    if bytes % format.frame_bytes() != 0 {
        return Err("incomplete WAVE frame");
    }
    Ok(Wave {
        format,
        offset,
        bytes,
    })
}
pub fn header(format: Format, bytes: u32) -> Result<[u8; 44], &'static str> {
    Format::new(format.rate, format.channels)?;
    if bytes % format.frame_bytes() as u32 != 0 {
        return Err("incomplete WAVE frame");
    }
    let riff = bytes.checked_add(36).ok_or("WAVE length overflow")?;
    let mut h = [0; 44];
    h[..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&riff.to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&u16::from(format.channels).to_le_bytes());
    h[24..28].copy_from_slice(&format.rate.to_le_bytes());
    h[28..32].copy_from_slice(&(format.rate * format.frame_bytes() as u32).to_le_bytes());
    h[32..34].copy_from_slice(&(format.frame_bytes() as u16).to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&bytes.to_le_bytes());
    Ok(h)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_header_roundtrips_and_truncation_fails() {
        let mut data = header(Format::SPEECH, 4).unwrap().to_vec();
        data.extend_from_slice(&[0, 0, 1, 0]);
        assert_eq!(
            parse(&data).unwrap(),
            Wave {
                format: Format::SPEECH,
                offset: 44,
                bytes: 4
            }
        );
        for len in 0..data.len() {
            assert!(parse(&data[..len]).is_err());
        }
    }
    #[test]
    fn odd_metadata_padding_and_bad_layout_are_checked() {
        let mut h = header(Format::SPEECH, 4).unwrap().to_vec();
        h.extend_from_slice(&[0; 4]);
        let mut data = h[..36].to_vec();
        data.extend_from_slice(b"JUNK\x01\0\0\0x\0");
        data.extend_from_slice(&h[36..]);
        let riff_len = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&riff_len.to_le_bytes());
        assert_eq!(parse(&data).unwrap().offset, 54);
        data[32] = 4;
        assert!(parse(&data).is_err());
    }
}
