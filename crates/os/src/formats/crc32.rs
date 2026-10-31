//! IEEE CRC-32, matching zlib and the existing upload/OTA contract.
/// zlib CRC-32 without the final inversion; start from `!0`.
pub(crate) fn update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    crc
}

pub fn crc32(data: &[u8]) -> u32 {
    !update(!0, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
}
