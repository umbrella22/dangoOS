//! A brief acoustic probe: quiet, 600 Hz, quiet, 1 kHz, quiet, at 16 kHz.
//! Recording the microphone independently also measures the room's noise floor.
pub const RATE: u32 = 16_000;
pub const FRAMES: u32 = 3 * RATE;

pub fn sample(frame: u32) -> i16 {
    let (start, hz) = match frame {
        8_000..20_000 => (8_000, 600),
        28_000..40_000 => (28_000, 1_000),
        _ => return 0,
    };
    let phase = ((frame - start) * hz) % RATE;
    let ramp = phase * 4 * 5_000 / RATE;
    let value = if ramp < 10_000 {
        ramp as i32 - 5_000
    } else {
        15_000 - ramp as i32
    };
    // 5 ms envelope avoids a step at either boundary of each tone.
    let edge = (frame - start).min(start + 12_000 - 1 - frame).min(80);
    (value * edge as i32 / 80) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_tones_have_silence_and_expected_periods() {
        assert!((0..8_000).all(|i| sample(i) == 0));
        assert!((20_000..28_000).all(|i| sample(i) == 0));
        assert!((40_000..FRAMES).all(|i| sample(i) == 0));
        for (start, period) in [(8_200, 80), (28_200, 16)] {
            assert!((start..start + 160).all(|i| sample(i) == sample(i + period)));
            assert!((start..start + period).any(|i| sample(i) > 0));
            assert!((start..start + period).any(|i| sample(i) < 0));
        }
    }
}
