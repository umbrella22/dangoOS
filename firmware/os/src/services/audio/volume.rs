//! Logical volume remains 0..100 across settings, Lua and AVRCP. The codec
//! maps that range to a protected DAC gain before enabling the speaker.
// ES8311 DAC register 0x32: 191 = 0 dB, one step = 0.5 dB.
const ZERO_DB_REGISTER: u8 = 191;
const MIN_DAC_REGISTER: u8 = 71; // -60 dB
/// Initial board calibration: old 75% = -15 dB, 3 dB below the reported
/// distortion onset at old 80%. Recheck with actual speaker/program material.
pub const OUTPUT_CEILING_DB: i16 = -15;
const _: () = assert!(OUTPUT_CEILING_DB >= -60 && OUTPUT_CEILING_DB <= 0);
const MAX_DAC_REGISTER: u8 = (ZERO_DB_REGISTER as i16 + 2 * OUTPUT_CEILING_DB) as u8;

pub fn dac_register(logical_percent: u8) -> Result<u8, &'static str> {
    if logical_percent > 100 {
        return Err("audio volume range");
    }
    Ok(MIN_DAC_REGISTER
        + (u16::from(logical_percent) * u16::from(MAX_DAC_REGISTER - MIN_DAC_REGISTER) / 100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_volume_sources_keep_full_logical_range_under_speaker_ceiling() {
        assert_eq!(dac_register(0), Ok(71));
        assert_eq!(dac_register(100), Ok(161));
        assert_eq!(191i16 - i16::from(dac_register(100).unwrap()), 30);
        let registers: alloc::vec::Vec<_> = (0..=100).map(|v| dac_register(v).unwrap()).collect();
        assert!(registers
            .windows(2)
            .all(|p| p[0] <= p[1] && p[1] - p[0] <= 1));
        assert!(registers.iter().all(|v| *v <= 161));
        // Upper logical range still changes gain, including remote 100%.
        assert!(registers[80] < registers[90] && registers[90] < registers[100]);
        for invalid in 101..=255 {
            assert!(dac_register(invalid).is_err());
        }
    }
}
