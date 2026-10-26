//! Slot EEPROM descriptor (V1, `0x86` bytes) as written by the official
//! `mosaico_module_mgr.c`.
//!
//! ```text
//! 0x00  "ESP" magic        0x03  board_type        ...
//! 0x34  CRC16 of 0x00..0x34
//! 0x36  manufacturing data 0x3E  CRC16 of 0x36..0x3E
//! 0x40  parameter block    0x42  parameter length (≤ 64)
//! 0x84  CRC16 of 0x40..0x84
//! ```
//!
//! Pure and dependency-free so `scripts/check/drivers.sh` can test it on the
//! host.

/// Bytes to read from the EEPROM.
pub const DESCRIPTOR_LEN: usize = 0x86;

/// `MOSAICO_BOARD_TYPE_CAMERA`.
pub const BOARD_TYPE_CAMERA: u8 = 0x07;
/// `MOSAICO_BOARD_TYPE_INTERACT`.
pub const BOARD_TYPE_INTERACT: u8 = 0x16;

const MAGIC: [u8; 3] = *b"ESP";
const BOARD_TYPE_OFFSET: usize = 0x03;
const DESC_CRC_OFFSET: usize = 0x34;
const MFG_DATA_OFFSET: usize = 0x36;
const MFG_CRC_OFFSET: usize = 0x3E;
const PARAM_DATA_OFFSET: usize = 0x40;
const PARAM_LENGTH_OFFSET: usize = 0x42;
const PARAM_CRC_OFFSET: usize = 0x84;
const PARAM_CAPACITY: usize = 64;

/// Why a descriptor was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    /// The first three bytes are not `"ESP"`.
    BadMagic([u8; 3]),
    /// A section CRC or the parameter length does not match. Two modules
    /// answering on the same address (camera fixed at `0x50` plus a board in
    /// the left slot) look like this.
    BadChecksum,
}

/// Validate a descriptor and return its `board_type`.
pub fn parse(raw: &[u8; DESCRIPTOR_LEN]) -> Result<u8, DescriptorError> {
    let magic = [raw[0], raw[1], raw[2]];
    if magic != MAGIC {
        return Err(DescriptorError::BadMagic(magic));
    }
    let section_ok =
        |data: core::ops::Range<usize>, crc_at: usize| read_le16(raw, crc_at) == crc16(&raw[data]);
    let param_length = read_le16(raw, PARAM_LENGTH_OFFSET) as usize;
    if section_ok(0..DESC_CRC_OFFSET, DESC_CRC_OFFSET)
        && section_ok(MFG_DATA_OFFSET..MFG_CRC_OFFSET, MFG_CRC_OFFSET)
        && section_ok(PARAM_DATA_OFFSET..PARAM_CRC_OFFSET, PARAM_CRC_OFFSET)
        && param_length <= PARAM_CAPACITY
    {
        Ok(raw[BOARD_TYPE_OFFSET])
    } else {
        Err(DescriptorError::BadChecksum)
    }
}

/// Human-readable `board_type` (official `mosaico_board_type_t`).
pub fn board_type_name(board_type: u8) -> &'static str {
    match board_type {
        0x01 => "Core",
        0x02 => "Power",
        0x03 => "Dock",
        0x04 => "Handle",
        0x05 => "Balance Car",
        0x06 => "Display",
        0x07 => "Camera",
        0x08 => "Sensor",
        0x09 => "IO Expansion",
        0x10 => "ToF",
        0x11 => "Matrix LED",
        0x12 => "Thermal",
        0x13 => "Relay",
        0x16 => "Interaction",
        _ => "Unknown",
    }
}

fn read_le16(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

/// CRC16, reflected polynomial `0xA001`, init `0xFFFF` (`mosaico_module_mgr.c`).
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in data {
        crc ^= byte as u16;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(raw: &mut [u8; DESCRIPTOR_LEN]) {
        for (data, crc_at) in [
            (0..DESC_CRC_OFFSET, DESC_CRC_OFFSET),
            (MFG_DATA_OFFSET..MFG_CRC_OFFSET, MFG_CRC_OFFSET),
            (PARAM_DATA_OFFSET..PARAM_CRC_OFFSET, PARAM_CRC_OFFSET),
        ] {
            let crc = crc16(&raw[data]).to_le_bytes();
            raw[crc_at..crc_at + 2].copy_from_slice(&crc);
        }
    }

    fn descriptor(board_type: u8) -> [u8; DESCRIPTOR_LEN] {
        let mut raw = [0u8; DESCRIPTOR_LEN];
        raw[..3].copy_from_slice(b"ESP");
        raw[BOARD_TYPE_OFFSET] = board_type;
        raw[0x10..0x1B].copy_from_slice(b"Interaction");
        raw[PARAM_LENGTH_OFFSET] = 8;
        seal(&mut raw);
        raw
    }

    #[test]
    fn crc16_matches_the_modbus_check_value() {
        // CRC-16/MODBUS of "123456789" is 0x4B37.
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }

    #[test]
    fn valid_descriptor_returns_its_board_type() {
        assert_eq!(parse(&descriptor(BOARD_TYPE_INTERACT)), Ok(0x16));
        assert_eq!(parse(&descriptor(BOARD_TYPE_CAMERA)), Ok(0x07));
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut raw = descriptor(BOARD_TYPE_INTERACT);
        raw[0] = 0xFF;
        assert_eq!(
            parse(&raw),
            Err(DescriptorError::BadMagic([0xFF, b'S', b'P']))
        );
    }

    #[test]
    fn any_corrupted_section_is_rejected() {
        for at in [0x05, 0x38, 0x50] {
            let mut raw = descriptor(BOARD_TYPE_INTERACT);
            raw[at] ^= 0x40;
            assert_eq!(
                parse(&raw),
                Err(DescriptorError::BadChecksum),
                "byte 0x{at:02X}"
            );
        }
    }

    #[test]
    fn oversized_parameter_block_is_rejected() {
        let mut raw = descriptor(BOARD_TYPE_INTERACT);
        raw[PARAM_LENGTH_OFFSET] = 65;
        seal(&mut raw);
        assert_eq!(parse(&raw), Err(DescriptorError::BadChecksum));
    }

    #[test]
    fn unknown_types_have_a_name() {
        assert_eq!(board_type_name(0x16), "Interaction");
        assert_eq!(board_type_name(0x42), "Unknown");
    }
}
