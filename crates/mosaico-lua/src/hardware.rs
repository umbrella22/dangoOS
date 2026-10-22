//! Snapshot-only hardware bridge. The firmware owns buses and output tasks.
//! Valid bits distinguish an unsampled device from a valid zero measurement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct HardwareSnapshot {
    /// Bit 0 IMU, 1 magnetometer, 2 gauge, 3 buttons, 4 LED, 5 haptic.
    pub available: u32,
    /// Bit 0 raw accel, 1 raw gyro, 2 temperature, 3 sensor time/status,
    /// 4 magnetic sample, 5 gauge sample, 6 debounced button levels.
    pub valid: u32,
    pub accel: [i32; 3],
    pub gyro: [i32; 3],
    pub temperature: i32,
    pub sensor_time: u32,
    pub imu_status: u32,
    pub magnetic: [f32; 3],
    pub magnet_status: u32,
    /// 0 away, 1 near, 2 contact (classification is independent of calibration).
    pub magnet_state: u32,
    pub voltage_mv: u32,
    pub current_ma: i32,
    pub soc: u32,
    pub charging: u32,
    /// Actual active slot's GPIOs; never AI/BOOT or arbitrary GPIO inputs.
    pub button_pins: [i32; 2],
    pub button_pressed: [u32; 2],
    pub led_generation: u32,
    pub button_generation: u32,
    /// Sequence of most recent edge; wrapping, starts at zero.
    pub button_sequence: u32,
    pub button_edges: [ButtonEdge; 16],
    /// Independent onboard buttons (AI GPIO7, BOOT GPIO61); never slot generations.
    pub onboard: OnboardButtons,
}

/// Fixed six-pixel GRB-strip frame; a request is staged by the Lua handle and
/// delivered atomically by refresh. A host must reject unavailable hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareRequest {
    Haptic(HapticPreset),
    LedFrame([[u8; 3]; 6]),
    LedRelease,
}

/// Existing affect-engine presets; no arbitrary duty cycle/duration is exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HapticPreset {
    Panic,
    Angry,
    Surprised,
    Puzzled,
    Happy,
    Satisfied,
    Confused,
    Listening,
    Thinking,
    Error,
    ModuleInsert,
    ModuleRemove,
    MagneticContact,
}
impl HapticPreset {
    pub const ALL: [Self; 13] = [
        Self::Panic,
        Self::Angry,
        Self::Surprised,
        Self::Puzzled,
        Self::Happy,
        Self::Satisfied,
        Self::Confused,
        Self::Listening,
        Self::Thinking,
        Self::Error,
        Self::ModuleInsert,
        Self::ModuleRemove,
        Self::MagneticContact,
    ];
}

/// Timestamped debounced edge, stored in a bounded ring (sequence % 16).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ButtonEdge {
    pub sequence: u32,
    pub at_ms: u32,
    pub key: u32,
    pub pressed: u32,
}
impl HardwareSnapshot {
    pub fn button_edge(&mut self, key: usize, pressed: bool, at_ms: u32) {
        self.button_pressed[key] = pressed as u32;
        self.button_sequence = self.button_sequence.wrapping_add(1);
        self.button_edges[(self.button_sequence % 16) as usize] = ButtonEdge {
            sequence: self.button_sequence,
            at_ms,
            key: key as u32,
            pressed: pressed as u32,
        };
    }
    /// Detach or active-slot change invalidates every existing button handle.
    pub fn button_board(&mut self, pins: Option<[i32; 2]>) {
        let next = pins.unwrap_or([-1; 2]);
        if self.button_pins != next || (self.available & 8 != 0) != pins.is_some() {
            self.button_generation = self.button_generation.wrapping_add(1);
            self.button_pins = next;
            self.button_pressed = [0; 2];
            self.valid &= !64;
        }
        if pins.is_some() {
            self.available |= 8;
        } else {
            self.available &= !8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamped_button_ring_wraps_and_board_changes_invalidate_handles() {
        let mut s = HardwareSnapshot::default();
        s.button_board(Some([11, 10]));
        let generation = s.button_generation;
        s.valid |= 64;
        s.button_edge(0, true, u32::MAX - 20);
        s.button_edge(0, false, 30);
        assert_eq!(s.button_edges[1].at_ms, u32::MAX - 20);
        assert_eq!(s.button_edges[2].at_ms, 30);
        for n in 0..18 {
            s.button_edge(1, n % 2 == 0, 31 + n);
        }
        assert_eq!(s.button_sequence, 20);
        assert_eq!(s.button_edges[4].sequence, 20);
        s.button_board(None);
        assert_eq!(s.available & 8, 0);
        assert_eq!(s.valid & 64, 0);
        assert_ne!(s.button_generation, generation);
        assert_eq!(s.button_pressed, [0; 2]);
        let generation = s.button_generation;
        s.button_board(Some([11, 10]));
        assert_ne!(s.button_generation, generation);
        assert_eq!(
            s.button_sequence, 20,
            "generation, not sequence reset, invalidates readers"
        );
    }
}

/// Independent stable button domain, merged by the firmware Lua host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct OnboardButtons {
    pub ready: u32,
    pub pressed: [u32; 2],
    pub sequence: u32,
    pub edges: [ButtonEdge; 16],
}
impl OnboardButtons {
    pub fn edge(&mut self, key: usize, pressed: bool, at_ms: u32) {
        self.pressed[key] = pressed as u32;
        self.sequence = self.sequence.wrapping_add(1);
        self.edges[(self.sequence % 16) as usize] = ButtonEdge {
            sequence: self.sequence,
            at_ms,
            key: key as u32,
            pressed: pressed as u32,
        };
    }
}
