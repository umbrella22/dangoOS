//! Fan calculations independent of the UI, clocks, and PWM hardware.

/// Automatic duty: 25% below 30°C, rising to 100% at 60°C.
/// NaN retains the existing full-duty fallback.
pub fn automatic_duty(temperature: f32) -> u8 {
    if temperature < 30.0 {
        25
    } else if temperature < 40.0 {
        25 + ((temperature - 30.0) * 2.5) as u8
    } else if temperature < 50.0 {
        50 + ((temperature - 40.0) * 2.5) as u8
    } else if temperature < 60.0 {
        75 + ((temperature - 50.0) * 2.5) as u8
    } else {
        100
    }
}

/// Display estimate for an 8W fan; duty is a percentage in 0..=100.
pub fn estimated_power(duty: u8) -> f32 {
    (duty as f32 / 100.0) * 8.0
}
