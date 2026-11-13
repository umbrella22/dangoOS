//! Magnetic approach / contact / release classifier (one BMM150).
//!
//! Pure logic, host-tested. The sensor task feeds every compensated sample
//! and publishes the returned events.
//!
//! Baseline: the board's own connector magnets give each sensor a large
//! static field (about 550 and 1700 uT on R1.1), so every threshold is a
//! delta from a learned baseline. As in the official
//! `mosaico_mag_classifier`, the baseline is only accepted from a quiet
//! 8-sample window, and it is re-learned whenever the static field changes
//! for a known reason (a module inserted or removed).

use crate::companion::render::math::inv_sqrt;

/// One compensated BMM150 sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MagReading {
    pub x_ut: f32,
    pub y_ut: f32,
    pub magnitude_ut: f32,
}

/// Baseline-relative magnetic classifier.
///
/// This is a provisional single-axis-pair classifier, **not** the two-sensor
/// `mosaico_interaction` edge classifier. It produces approach/contact/release
/// from the field magnitude and orbit/edge-change from the direction of the
/// XY field vector. Every event carries an internal confidence (0..=100); the
/// plan's `>= 0.5` gate is applied before an event is returned. H2 calibration
/// replaces these thresholds and H2's two-sensor topology work replaces the
/// single-sensor orbit heuristic.
pub struct MagneticClassifier {
    baseline_ut: f32,
    /// Collecting a quiet window before trusting the baseline.
    calibrating: bool,
    window: [f32; MAG_CAL_SAMPLES],
    window_len: usize,
    rejections: u16,
    approaching: bool,
    contacted: bool,
    contact_votes: u8,
    release_votes: u8,
    prev_xy: Option<[f32; 2]>,
    orbit_sign: i8,
    orbit_accum: f32,
    orbit_samples: u8,
    last_orbit_ms: u64,
    last_edge_ms: u64,
}

/// A confirmed magnetic transition. `confidence` is only carried by the
/// approach/direction events because contact/release are vote-confirmed edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MagneticEvent {
    Approach { confidence: u8 },
    Contact,
    Release,
    Orbit { confidence: u8 },
    Edge { confidence: u8 },
}

// Provisional H1.2 thresholds. RFC 0004 section 4.6 does not specify numeric
// values beyond the confidence gate; these are calibration points for H2.
const MAG_APPROACH_DELTA_UT: f32 = 25.0;
const MAG_CONTACT_DELTA_UT: f32 = 80.0;
const MAG_RELEASE_DELTA_UT: f32 = 35.0;
const MAG_BASELINE_TRACK_DELTA_UT: f32 = 15.0;
const MAG_BASELINE_ALPHA: f32 = 0.02;
const MAG_CONTACT_VOTES: u8 = 2;
const MAG_RELEASE_VOTES: u8 = 3;
/// Plan gate: confidence >= 0.5 is accepted.
const MAG_CONFIDENCE_MIN: u8 = 50;
/// A direction change must exceed this per-sample sine to count as rotation.
const MAG_ORBIT_SIN_MIN: f32 = 0.15;
/// Accumulated sine over an orbit window (about 45 degrees).
const MAG_ORBIT_ACCUM_MIN: f32 = 0.80;
const MAG_ORBIT_SAMPLES: u8 = 4;
const MAG_ORBIT_COOLDOWN_MS: u64 = 600;
/// cos(60 degrees): a larger direction jump is an edge-change candidate.
const MAG_EDGE_DOT_MAX: f32 = 0.50;
const MAG_EDGE_COOLDOWN_MS: u64 = 800;
const MAG_MIN_XY_UT: f32 = 8.0;
/// Baseline window (official profile: `filter_samples = 8`), 0.4 s at 20 Hz.
pub const MAG_CAL_SAMPLES: usize = 8;
/// Largest spread inside the window still called quiet. The sensors read
/// about +-3 uT of noise at rest; a hand-held magnet moves tens of uT.
pub const MAG_CAL_MAX_SPAN_UT: f32 = 12.0;

impl MagneticClassifier {
    pub const fn new() -> Self {
        Self {
            baseline_ut: 0.0,
            calibrating: true,
            window: [0.0; MAG_CAL_SAMPLES],
            window_len: 0,
            rejections: 0,
            approaching: false,
            contacted: false,
            contact_votes: 0,
            release_votes: 0,
            prev_xy: None,
            orbit_sign: 0,
            orbit_accum: 0.0,
            orbit_samples: 0,
            last_orbit_ms: 0,
            last_edge_ms: 0,
        }
    }

    pub fn baseline_ut(&self) -> f32 {
        self.baseline_ut
    }

    /// `false` while the classifier is waiting for a quiet window.
    pub fn is_calibrated(&self) -> bool {
        !self.calibrating
    }

    /// Windows rejected because the field was moving (a magnet or a hand).
    pub fn rejections(&self) -> u16 {
        self.rejections
    }

    /// Forget the baseline and learn a new one, as the official classifier
    /// does at boot. Call after the static field changed for a known reason,
    /// such as a module (with its own magnets) being inserted or removed.
    /// No release is reported for a magnet that was attached at that moment.
    pub fn recalibrate(&mut self) {
        let (last_orbit_ms, last_edge_ms) = (self.last_orbit_ms, self.last_edge_ms);
        *self = Self::new();
        self.last_orbit_ms = last_orbit_ms;
        self.last_edge_ms = last_edge_ms;
    }

    /// Slide an 8-sample window until its span is small enough, then take the
    /// mean as the baseline (official `mosaico_mag_classifier`: 8-sample
    /// filter, baseline rejected while the field moves).
    fn calibrate(&mut self, sample: &MagReading) {
        if self.window_len == MAG_CAL_SAMPLES {
            self.window.copy_within(1.., 0);
            self.window_len -= 1;
        }
        self.window[self.window_len] = sample.magnitude_ut;
        self.window_len += 1;
        if self.window_len < MAG_CAL_SAMPLES {
            return;
        }

        let (mut low, mut high, mut sum) = (f32::MAX, f32::MIN, 0.0);
        for &value in &self.window {
            low = low.min(value);
            high = high.max(value);
            sum += value;
        }
        if high - low > MAG_CAL_MAX_SPAN_UT {
            self.rejections = self.rejections.saturating_add(1);
            return;
        }
        self.baseline_ut = sum / MAG_CAL_SAMPLES as f32;
        self.calibrating = false;
        self.prev_xy = Some([sample.x_ut, sample.y_ut]);
    }

    pub fn update(&mut self, sample: &MagReading, now_ms: u64) -> Option<MagneticEvent> {
        if sample.magnitude_ut <= 0.0 {
            return None;
        }

        if self.calibrating {
            self.calibrate(sample);
            return None;
        }

        let delta = sample.magnitude_ut - self.baseline_ut;
        let mut event = None;

        if delta > MAG_APPROACH_DELTA_UT {
            self.release_votes = 0;
            if !self.contacted {
                if delta > MAG_CONTACT_DELTA_UT {
                    self.contact_votes = self.contact_votes.saturating_add(1);
                    if self.contact_votes >= MAG_CONTACT_VOTES {
                        self.contacted = true;
                        self.approaching = false;
                        self.contact_votes = 0;
                        event = Some(MagneticEvent::Contact);
                    }
                } else {
                    self.contact_votes = 0;
                    if !self.approaching {
                        let confidence = delta_confidence(delta, MAG_APPROACH_DELTA_UT);
                        if confidence >= MAG_CONFIDENCE_MIN {
                            self.approaching = true;
                            event = Some(MagneticEvent::Approach { confidence });
                        }
                    }
                }
            }
        } else {
            self.contact_votes = 0;
            if self.contacted {
                self.release_votes = self.release_votes.saturating_add(1);
                if self.release_votes >= MAG_RELEASE_VOTES && delta < MAG_RELEASE_DELTA_UT {
                    self.contacted = false;
                    self.approaching = false;
                    self.release_votes = 0;
                    event = Some(MagneticEvent::Release);
                }
            } else {
                self.release_votes = 0;
                if self.approaching && delta < MAG_RELEASE_DELTA_UT {
                    self.approaching = false;
                }
            }

            // Track the ambient field only while no magnet is near.
            if delta.abs() < MAG_BASELINE_TRACK_DELTA_UT {
                self.baseline_ut += (sample.magnitude_ut - self.baseline_ut) * MAG_BASELINE_ALPHA;
            }
        }

        // Orbit and edge-change need the field to be clearly above ambient.
        if event.is_none() && delta > MAG_APPROACH_DELTA_UT {
            event = self.direction_event(sample, now_ms);
        }

        self.prev_xy = Some([sample.x_ut, sample.y_ut]);
        event
    }

    /// Detect a sustained XY rotation (orbit) or a sudden direction jump
    /// (edge change). A rotation of the whole device looks the same to one
    /// magnetometer; H2 must combine this with IMU attitude before enabling
    /// orbit reactions.
    fn direction_event(&mut self, sample: &MagReading, now_ms: u64) -> Option<MagneticEvent> {
        let prev = self.prev_xy?;
        let prev_norm_sq = prev[0] * prev[0] + prev[1] * prev[1];
        let norm_sq = sample.x_ut * sample.x_ut + sample.y_ut * sample.y_ut;
        if prev_norm_sq < MAG_MIN_XY_UT * MAG_MIN_XY_UT || norm_sq < MAG_MIN_XY_UT * MAG_MIN_XY_UT {
            return None;
        }

        let product = prev_norm_sq * norm_sq;
        let norm = product * inv_sqrt(product);
        if norm <= 0.0 {
            return None;
        }

        let cross = prev[0] * sample.y_ut - prev[1] * sample.x_ut;
        let dot = prev[0] * sample.x_ut + prev[1] * sample.y_ut;
        let sin = cross / norm;
        let cos = dot / norm;

        if sin.abs() >= MAG_ORBIT_SIN_MIN {
            let sign = if sin > 0.0 { 1 } else { -1 };
            if self.orbit_sign != sign {
                self.orbit_sign = sign;
                self.orbit_accum = 0.0;
                self.orbit_samples = 0;
            }
            self.orbit_accum += sin;
            self.orbit_samples = self.orbit_samples.saturating_add(1);

            if self.orbit_samples >= MAG_ORBIT_SAMPLES
                && self.orbit_accum.abs() >= MAG_ORBIT_ACCUM_MIN
            {
                let confidence = ((self.orbit_accum.abs() * 100.0) as u32).min(100) as u8;
                let ready = now_ms >= self.last_orbit_ms.saturating_add(MAG_ORBIT_COOLDOWN_MS);
                self.orbit_accum = 0.0;
                self.orbit_samples = 0;
                if confidence >= MAG_CONFIDENCE_MIN && ready {
                    self.last_orbit_ms = now_ms;
                    return Some(MagneticEvent::Orbit { confidence });
                }
            }
        } else {
            self.orbit_accum *= 0.5;
        }

        if cos < MAG_EDGE_DOT_MAX {
            let confidence = (((1.0 - cos) * 100.0) as u32).min(100) as u8;
            let ready = now_ms >= self.last_edge_ms.saturating_add(MAG_EDGE_COOLDOWN_MS);
            if confidence >= MAG_CONFIDENCE_MIN && ready {
                self.last_edge_ms = now_ms;
                return Some(MagneticEvent::Edge { confidence });
            }
        }

        None
    }
}

/// Map a delta to 0..=100 such that the trigger threshold is exactly 50, the
/// plan's confidence gate.
fn delta_confidence(delta: f32, threshold: f32) -> u8 {
    if threshold <= 0.0 {
        return 0;
    }
    ((delta / threshold) * 50.0).clamp(0.0, 100.0) as u8
}

impl Default for MagneticClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(magnitude_ut: f32) -> MagReading {
        MagReading {
            x_ut: magnitude_ut * 0.8,
            y_ut: magnitude_ut * 0.6,
            magnitude_ut,
        }
    }

    fn feed(
        classifier: &mut MagneticClassifier,
        values: &[f32],
        t: &mut u64,
    ) -> Vec<MagneticEvent> {
        let mut events = Vec::new();
        for &value in values {
            *t += 50;
            events.extend(classifier.update(&at(value), *t));
        }
        events
    }

    #[test]
    fn baseline_waits_for_a_quiet_window() {
        let mut classifier = MagneticClassifier::new();
        let mut t = 0;
        // A magnet moving at boot: no baseline, no events.
        let moving: Vec<f32> = (0..20).map(|i| 550.0 + (i % 5) as f32 * 30.0).collect();
        assert!(feed(&mut classifier, &moving, &mut t).is_empty());
        assert!(!classifier.is_calibrated());
        assert!(classifier.rejections() > 0);

        assert!(feed(&mut classifier, &[552.0; MAG_CAL_SAMPLES], &mut t).is_empty());
        assert!(classifier.is_calibrated());
        assert!((classifier.baseline_ut() - 552.0).abs() < 0.5);
    }

    #[test]
    fn contact_and_release_relative_to_the_baseline() {
        let mut classifier = MagneticClassifier::new();
        let mut t = 0;
        feed(&mut classifier, &[1700.0; MAG_CAL_SAMPLES], &mut t);
        let events = feed(&mut classifier, &[1900.0, 1900.0, 1900.0], &mut t);
        assert!(events.contains(&MagneticEvent::Contact));
        let events = feed(&mut classifier, &[1701.0; 4], &mut t);
        assert!(events.contains(&MagneticEvent::Release));
    }

    #[test]
    fn a_module_magnet_is_absorbed_by_recalibration() {
        let mut classifier = MagneticClassifier::new();
        let mut t = 0;
        feed(&mut classifier, &[550.0; MAG_CAL_SAMPLES], &mut t);

        // A module adds a static +60 uT. Without a new baseline that sits
        // between approach and contact for ever.
        classifier.recalibrate();
        assert!(feed(&mut classifier, &[610.0; 20], &mut t).is_empty());
        assert!((classifier.baseline_ut() - 610.0).abs() < 0.5);

        // A real magnet is still detected on top of the new field.
        let events = feed(&mut classifier, &[800.0, 800.0, 800.0], &mut t);
        assert!(events.contains(&MagneticEvent::Contact));
    }
}
