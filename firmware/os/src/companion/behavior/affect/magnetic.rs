//! Magnetic approach, contact and edge reactions.
use super::*;

impl AffectEngine {
    pub(super) fn on_magnetic_approach(&mut self, at: u64) {
        if self.rouse(at, Rouse::Gentle) {
            return;
        }
        let _ = self.try_start_reaction(
            Expression::Curious,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_APPROACH_MS,
            None,
        );
    }
    pub(super) fn on_magnetic_contact(&mut self, at: u64) {
        self.keep_awake(at);
        if self.magnetic_contacted {
            return;
        }
        self.magnetic_contacted = true;
        let _ = self.try_start_reaction(
            Expression::Satisfied,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_CONTACT_MS,
            Some(HapticPattern::MagneticContact),
        );
    }
    pub(super) fn on_magnetic_release(&mut self, at: u64) {
        if !self.magnetic_contacted {
            return;
        }
        self.magnetic_contacted = false;
        let _ = self.try_start_reaction(
            Expression::Confused,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_RELEASE_MS,
            Some(HapticPattern::Confused),
        );
    }
    pub(super) fn on_magnetic_orbit(&mut self, at: u64) {
        let _ = self.try_start_reaction(
            Expression::Surprised,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_ORBIT_MS,
            Some(HapticPattern::Surprised),
        );
    }
    pub(super) fn on_magnetic_edge(&mut self, at: u64, confidence: u8) {
        if confidence < BMM_CONFIDENCE_MIN {
            return;
        }
        let _ = self.try_start_reaction(
            Expression::Puzzled,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_EDGE_MS,
            Some(HapticPattern::Puzzled),
        );
    }
}
