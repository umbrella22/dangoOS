//! The latest value of every sensor, for the sensors page.
//!
//! Built from the same [`SensorEvent`] stream the affect engine reads, so the
//! page shows exactly what the fox reacts to.

use crate::behavior::affect::{EventKind, EventSource, ModuleKind, SensorEvent};
use crate::fox::math::{inv_sqrt, round, PI};

/// What one slot holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SlotState {
    #[default]
    Empty,
    Module(ModuleKind),
}

/// The magnet as the classifier last reported it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MagnetState {
    #[default]
    Away,
    Near,
    Contact,
}

/// Latest sensor values. `None` until the first sample arrives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemSnapshot {
    /// Milli-g.
    pub accel_mg: Option<[i16; 3]>,
    /// Degrees per second.
    pub gyro_dps: Option<[i16; 3]>,
    /// Relative light `0..=100`.
    pub light: Option<u8>,
    pub presence: Option<bool>,
    /// State of charge `0..=100`.
    pub battery: Option<u8>,
    pub left: SlotState,
    pub right: SlotState,
    pub magnet: MagnetState,
}

impl SystemSnapshot {
    pub const fn new() -> Self {
        Self {
            accel_mg: None,
            gyro_dps: None,
            light: None,
            presence: None,
            battery: None,
            left: SlotState::Empty,
            right: SlotState::Empty,
            magnet: MagnetState::Away,
        }
    }

    /// Fold one event in. Returns whether a shown value changed.
    pub fn observe(&mut self, event: &SensorEvent) -> bool {
        let before = *self;
        match event.kind {
            EventKind::Accel {
                ax_mg,
                ay_mg,
                az_mg,
            } => self.accel_mg = Some([ax_mg, ay_mg, az_mg]),
            EventKind::Gyro {
                gx_dps,
                gy_dps,
                gz_dps,
            } => self.gyro_dps = Some([gx_dps, gy_dps, gz_dps]),
            EventKind::LightLevel(level) => self.light = Some(level),
            EventKind::Presence(present) => self.presence = Some(present),
            EventKind::BatteryLevel(level) => self.battery = Some(level),
            EventKind::ModuleInserted(kind) => *self.slot(event.source) = SlotState::Module(kind),
            EventKind::ModuleRemoved(_) => *self.slot(event.source) = SlotState::Empty,
            EventKind::MagneticApproach | EventKind::MagneticOrbit => {
                if self.magnet == MagnetState::Away {
                    self.magnet = MagnetState::Near;
                }
            }
            EventKind::MagneticContact => self.magnet = MagnetState::Contact,
            EventKind::MagneticRelease => self.magnet = MagnetState::Away,
            _ => {}
        }
        *self != before
    }

    fn slot(&mut self, source: EventSource) -> &mut SlotState {
        match source {
            EventSource::ModuleLeft => &mut self.left,
            _ => &mut self.right,
        }
    }

    /// Tilt from vertical, whole degrees (0 = lying flat, face up).
    pub fn tilt_deg(&self) -> Option<u32> {
        let [x, y, z] = self.accel_mg?;
        let (x, y, z) = (x as f32, y as f32, z as f32);
        let h2 = x * x + y * y;
        let horizontal = if h2 > 0.0 { h2 * inv_sqrt(h2) } else { 0.0 };
        let degrees = angle_from_vertical(horizontal, z) * (180.0 / PI);
        Some(round(degrees).max(0) as u32)
    }
}

/// `atan2(h, z)` for `h >= 0`, in `0..=PI`. The polynomial `atan` is within
/// 0.3 degrees, plenty for a readout.
fn angle_from_vertical(h: f32, z: f32) -> f32 {
    fn atan_unit(t: f32) -> f32 {
        // atan(t) for t in 0..=1.
        t * (PI / 4.0) + 0.273 * t * (1.0 - t)
    }
    let az = if z < 0.0 { -z } else { z };
    if h == 0.0 && az == 0.0 {
        return 0.0;
    }
    // Angle between the vector and +z measured in the first quadrant.
    let first = if h <= az {
        atan_unit(h / az)
    } else {
        PI / 2.0 - atan_unit(az / h)
    };
    if z >= 0.0 {
        first
    } else {
        PI - first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(source: EventSource, kind: EventKind) -> SensorEvent {
        SensorEvent {
            at_ms: 0,
            source,
            kind,
        }
    }

    #[test]
    fn values_start_unknown() {
        let s = SystemSnapshot::new();
        assert_eq!(s.accel_mg, None);
        assert_eq!(s.tilt_deg(), None);
        assert_eq!(s.left, SlotState::Empty);
    }

    #[test]
    fn events_update_the_matching_field() {
        let mut s = SystemSnapshot::new();
        assert!(s.observe(&event(EventSource::Light, EventKind::LightLevel(42))));
        assert!(
            !s.observe(&event(EventSource::Light, EventKind::LightLevel(42))),
            "unchanged"
        );
        assert!(s.observe(&event(EventSource::Battery, EventKind::BatteryLevel(87))));
        assert!(s.observe(&event(
            EventSource::ModuleRight,
            EventKind::ModuleInserted(ModuleKind::Interaction)
        )));
        assert_eq!(s.light, Some(42));
        assert_eq!(s.battery, Some(87));
        assert_eq!(s.right, SlotState::Module(ModuleKind::Interaction));
        assert_eq!(s.left, SlotState::Empty);
        s.observe(&event(
            EventSource::ModuleRight,
            EventKind::ModuleRemoved(ModuleKind::Interaction),
        ));
        assert_eq!(s.right, SlotState::Empty);
    }

    #[test]
    fn the_magnet_follows_contact_and_release() {
        let mut s = SystemSnapshot::new();
        s.observe(&event(EventSource::Bmm150, EventKind::MagneticApproach));
        assert_eq!(s.magnet, MagnetState::Near);
        s.observe(&event(EventSource::Bmm150, EventKind::MagneticContact));
        s.observe(&event(EventSource::Bmm150, EventKind::MagneticOrbit));
        assert_eq!(
            s.magnet,
            MagnetState::Contact,
            "orbit does not downgrade contact"
        );
        s.observe(&event(EventSource::Bmm150, EventKind::MagneticRelease));
        assert_eq!(s.magnet, MagnetState::Away);
    }

    #[test]
    fn tilt_is_zero_flat_and_ninety_on_edge() {
        let mut s = SystemSnapshot::new();
        let accel = |x, y, z| EventKind::Accel {
            ax_mg: x,
            ay_mg: y,
            az_mg: z,
        };
        s.observe(&event(EventSource::Imu, accel(0, 0, 1000)));
        assert_eq!(s.tilt_deg(), Some(0));
        s.observe(&event(EventSource::Imu, accel(1000, 0, 0)));
        assert_eq!(s.tilt_deg(), Some(90));
        s.observe(&event(EventSource::Imu, accel(0, 707, 707)));
        assert_eq!(s.tilt_deg(), Some(45));
    }
}

/// Lua hardware snapshot is separate from UI display units: raw BMI270 counts,
/// compensated BMM150 microteslas, and BQ27220 electrical measurements.
/// Producers fill this in their own task; consumers only copy it.
pub use mosaico_lua::{ButtonEdge, HardwareSnapshot};

/// Record the chosen board magnetometer's compensated sample and classifier
/// edge together. The status byte is BMM150 register 0x4A, not its DRDY bit.
pub fn record_magnetic(
    snapshot: &mut HardwareSnapshot,
    magnetic: [f32; 3],
    status: u8,
    event: Option<crate::behavior::magnet::MagneticEvent>,
) {
    use crate::behavior::magnet::MagneticEvent;
    snapshot.magnetic = magnetic;
    snapshot.magnet_status = status as u32;
    snapshot.valid |= 16;
    match event {
        Some(MagneticEvent::Approach { .. } | MagneticEvent::Orbit { .. }) => {
            if snapshot.magnet_state == 0 {
                snapshot.magnet_state = 1;
            }
        }
        Some(MagneticEvent::Contact) => snapshot.magnet_state = 2,
        Some(MagneticEvent::Release) => snapshot.magnet_state = 0,
        _ => {}
    }
}

/// A failed transaction invalidates the sample; it must not become a plausible
/// zero field or an indefinitely readable stale measurement.
pub fn invalidate_magnetic(snapshot: &mut HardwareSnapshot) {
    snapshot.valid &= !16;
}

#[cfg(test)]
mod hardware_tests {
    use super::*;
    use crate::behavior::magnet::MagneticEvent;
    #[test]
    fn magnetic_producer_retains_zero_and_real_status_then_invalidates_errors() {
        let mut snapshot = HardwareSnapshot::default();
        record_magnetic(
            &mut snapshot,
            [0.0, 0.0, 0.0],
            0x42,
            Some(MagneticEvent::Approach { confidence: 100 }),
        );
        assert_eq!(snapshot.valid & 16, 16);
        assert_eq!(snapshot.magnetic, [0.0; 3]);
        assert_eq!(snapshot.magnet_status, 0x42);
        assert_eq!(snapshot.magnet_state, 1);
        record_magnetic(&mut snapshot, [0.0; 3], 0, Some(MagneticEvent::Contact));
        record_magnetic(
            &mut snapshot,
            [0.0; 3],
            0,
            Some(MagneticEvent::Orbit { confidence: 100 }),
        );
        assert_eq!(snapshot.magnet_state, 2);
        invalidate_magnetic(&mut snapshot);
        assert_eq!(snapshot.valid & 16, 0);
        record_magnetic(&mut snapshot, [1.0; 3], 0, Some(MagneticEvent::Release));
        assert_eq!(snapshot.magnet_state, 0);
        assert_eq!(snapshot.valid & 16, 16);
    }
}
