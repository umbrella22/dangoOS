//! BMI270 FIFO watermark interrupt contract, independent of I2C/GPIO backends.
//!
//! Register definitions: Bosch BMI270 SensorAPI, commit
//! 41129fcfe39c583ee5462d79195741945d51c1fe, bmi2_defs.h and bmi2.c.
//! GPIO2 is shared; a low level alone is never proof of a BMI270 event.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Route {
    Off = 0,
    Int1 = 1,
    Int2 = 2,
}
impl Route {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "int1" => Some(Self::Int1),
            "int2" => Some(Self::Int2),
            _ => None,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Int1 => "int1",
            Self::Int2 => "int2",
        }
    }
    /// Disconnect routing before changing electrical mode and watermark.
    /// INT IO = output enabled, open drain, active low, input disabled (0Ch).
    /// Both unused pins stay disabled. 12 bytes = one headerless accel+gyro frame.
    pub const fn registers(self) -> [(u8, u8); 7] {
        [
            (0x58, 0),
            (0x53, if matches!(self, Self::Int1) { 0x0c } else { 0 }),
            (0x54, if matches!(self, Self::Int2) { 0x0c } else { 0 }),
            (0x55, 0),
            (0x46, 12),
            (0x47, 0),
            (
                0x58,
                match self {
                    Self::Off => 0,
                    Self::Int1 => 2,
                    Self::Int2 => 0x20,
                },
            ),
        ]
    }
}

pub const STATUS_REGISTER: u8 = 0x1c;
pub const WATERMARK: u16 = 0x0200;
/// Five 50 Hz samples; below the driver's 16-frame batch and 2 KiB FIFO.
pub const FALLBACK_MS: u64 = 100;

pub struct DrainPolicy {
    last_drain_ms: u64,
}
impl Default for DrainPolicy {
    fn default() -> Self {
        Self::new()
    }
}
impl DrainPolicy {
    pub const fn new() -> Self {
        Self { last_drain_ms: 0 }
    }
    pub fn due(&self, now_ms: u64, enabled: bool, status: u16) -> bool {
        !enabled
            || status & WATERMARK != 0
            || now_ms.saturating_sub(self.last_drain_ms) >= FALLBACK_MS
    }
    pub fn drained(&mut self, now_ms: u64) {
        self.last_drain_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_interrupts_do_not_trigger_an_imu_read_but_fallback_is_bounded() {
        let mut p = DrainPolicy::new();
        p.drained(1000);
        assert!(!p.due(1020, true, 0));
        assert!(!p.due(1099, true, 0x8000));
        assert!(p.due(1100, true, 0));
        assert!(p.due(1020, true, WATERMARK));
        assert!(p.due(1020, false, 0));
    }
    #[test]
    fn routing_never_enables_push_pull_on_a_shared_line() {
        for route in [Route::Off, Route::Int1, Route::Int2] {
            let seq = route.registers();
            assert_eq!(seq[0], (0x58, 0));
            for (reg, value) in seq {
                if reg == 0x53 || reg == 0x54 {
                    assert!(value == 0 || value == 0x0c);
                }
            }
            assert_eq!(
                seq[6].1,
                match route {
                    Route::Off => 0,
                    Route::Int1 => 2,
                    Route::Int2 => 0x20,
                }
            );
        }
    }
}
