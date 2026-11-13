//! Monotonic device time shared by input and the UI runtime.
pub fn now_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}
