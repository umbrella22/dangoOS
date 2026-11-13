//! Module arrival/removal sequences and staged advancement.
use super::*;

impl AffectEngine {
    pub(super) fn advance_module_sequence(&mut self, now: u64) {
        let mut sequence = match self.module_sequence {
            Some(sequence) => sequence,
            None => return,
        };
        loop {
            let step = sequence.steps[sequence.index as usize];
            let end = sequence.stage_started_ms.saturating_add(step.duration_ms);
            if now < end {
                break;
            }
            sequence.stage_started_ms = end;
            sequence.index += 1;
            if sequence.index >= sequence.len {
                self.module_sequence = None;
                self.fallback_since_ms = end;
                return;
            }
        }
        self.module_sequence = Some(sequence);
    }
    /// Slot presence flag for `source`, or `None` for a non-slot source.
    pub(super) fn slot_present(&mut self, source: EventSource) -> Option<&mut bool> {
        match source {
            EventSource::ModuleLeft => Some(&mut self.module_left_present),
            EventSource::ModuleRight => Some(&mut self.module_right_present),
            _ => None,
        }
    }
    pub(super) fn on_module_inserted(&mut self, at: u64, source: EventSource, kind: ModuleKind) {
        // Plugging a module in is an interaction; its own sequence is the
        // wake face. Removing one is not: without the board she may doze off.
        self.keep_awake(at);
        // The sensor task re-learns the magnetometer baselines after a module
        // change and reports no release for a magnet attached at that time.
        self.magnetic_contacted = false;
        match self.slot_present(source) {
            Some(present) if !*present => *present = true,
            _ => return,
        }
        match kind {
            ModuleKind::Interaction => {
                let steps = [
                    ModuleStep {
                        expression: Expression::Surprised,
                        reason: ReactionReason::ModuleInsert,
                        duration_ms: INTERACTION_INSERT_SURPRISED_MS,
                    },
                    ModuleStep {
                        expression: Expression::Curious,
                        reason: ReactionReason::ModuleInsert,
                        duration_ms: INTERACTION_INSERT_CURIOUS_MS,
                    },
                    ModuleStep {
                        expression: Expression::Happy,
                        reason: ReactionReason::ModuleInsert,
                        duration_ms: INTERACTION_INSERT_HAPPY_MS,
                    },
                ];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleInsert);
            }
            ModuleKind::Camera => {
                let steps = [ModuleStep {
                    expression: Expression::Curious,
                    reason: ReactionReason::ModuleInsert,
                    duration_ms: CAMERA_INSERT_CURIOUS_MS,
                }];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleInsert);
            }
        }
    }
    pub(super) fn on_module_removed(&mut self, at: u64, source: EventSource, kind: ModuleKind) {
        self.magnetic_contacted = false;
        match self.slot_present(source) {
            Some(present) if *present => *present = false,
            _ => return,
        }
        match kind {
            ModuleKind::Interaction => {
                let steps = [
                    ModuleStep {
                        expression: Expression::Surprised,
                        reason: ReactionReason::ModuleRemove,
                        duration_ms: INTERACTION_REMOVE_SURPRISED_MS,
                    },
                    ModuleStep {
                        expression: Expression::Confused,
                        reason: ReactionReason::ModuleRemove,
                        duration_ms: INTERACTION_REMOVE_CONFUSED_MS,
                    },
                ];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleRemove);
            }
            ModuleKind::Camera => {
                let steps = [ModuleStep {
                    expression: Expression::Confused,
                    reason: ReactionReason::ModuleRemove,
                    duration_ms: CAMERA_REMOVE_CONFUSED_MS,
                }];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleRemove);
            }
        }
    }
    pub(super) fn start_module_sequence(
        &mut self,
        at: u64,
        steps: &[ModuleStep],
        haptic: HapticPattern,
    ) {
        if self.error_active {
            return;
        }
        if self.module_sequence.is_some() {
            return;
        }
        if let Some(current) = self.reaction {
            if at < current.ends_ms && current.priority > PRIO_PHYSICAL {
                return;
            }
        }

        self.reaction = None;
        let mut sequence = ModuleSequence {
            steps: [ModuleStep::EMPTY; 3],
            len: steps.len().min(3) as u8,
            index: 0,
            stage_started_ms: at,
        };
        let mut index = 0;
        while index < sequence.len as usize {
            sequence.steps[index] = steps[index];
            index += 1;
        }
        self.module_sequence = Some(sequence);
        self.queue_haptic(haptic);
    }
}
