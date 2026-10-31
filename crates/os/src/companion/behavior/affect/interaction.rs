//! Touch, keys and physical button reactions.
use super::*;

impl AffectEngine {
    pub(super) fn on_touch_pet(&mut self, at: u64) {
        if self.rouse(at, Rouse::Gentle) {
            return;
        }
        if let Some(reaction) = self.reaction {
            if reaction.reason == ReactionReason::TouchPet
                && reaction.expression == Expression::Happy
                && at < reaction.ends_ms
            {
                if let Some(active) = &mut self.reaction {
                    active.ends_ms = at.saturating_add(TOUCH_PET_HOLD_MS);
                }
                return;
            }
        }
        let _ = self.try_start_reaction(
            Expression::Happy,
            ReactionReason::TouchPet,
            PRIO_PHYSICAL,
            at,
            TOUCH_PET_HOLD_MS,
            Some(HapticPattern::Happy),
        );
    }
    pub(super) fn on_touch_poke(&mut self, at: u64) {
        if self.rouse(at, Rouse::Poke) {
            return;
        }
        push_window(
            &mut self.poke_times,
            &mut self.poke_count,
            at,
            RAPID_POKE_WINDOW_MS,
        );

        if self.poke_count >= RAPID_POKE_COUNT {
            self.poke_count = 0;
            if at >= self.angry_cooldown_until
                && !self.is_panic_active(at)
                && !self.is_angry_active(at)
            {
                self.start_angry(at, ReactionReason::TouchPoke);
                return;
            }
        }

        let _ = self.try_start_reaction(
            Expression::Puzzled,
            ReactionReason::TouchPoke,
            PRIO_PHYSICAL,
            at,
            TOUCH_POKE_MS,
            Some(HapticPattern::Puzzled),
        );
    }
    pub(super) fn on_key_down(&mut self, at: u64, source: EventSource) {
        if matches!(source, EventSource::KeyLeft | EventSource::KeyRight)
            && self.rouse(at, Rouse::Gentle)
        {
            return;
        }
        match source {
            EventSource::KeyLeft => self.on_key_left_down(at),
            EventSource::KeyRight => self.on_key_right_down(at),
            _ => {}
        }
    }
    pub(super) fn on_key_up(&mut self, at: u64, source: EventSource) {
        match source {
            EventSource::KeyLeft => {
                if !self.key_left_down {
                    return;
                }
                self.key_left_down = false;
                self.key_left_down_at = None;
                if let Some(reaction) = &mut self.reaction {
                    if reaction.reason == ReactionReason::KeyLeft
                        && reaction.expression == Expression::Happy
                        && reaction.priority == PRIO_PHYSICAL
                    {
                        reaction.ends_ms = at.saturating_add(KEY_HAPPY_MS);
                    }
                }
            }
            EventSource::KeyRight => {
                if !self.key_right_down {
                    return;
                }
                self.key_right_down = false;
                self.key_right_down_at = None;
            }
            _ => {}
        }
    }
    pub(super) fn on_key_left_down(&mut self, at: u64) {
        if self.key_left_down {
            return;
        }
        self.key_left_down = true;
        self.key_left_down_at = Some(at);

        if self.key_right_down {
            if let Some(right_at) = self.key_right_down_at {
                if at.saturating_sub(right_at) <= HUG_WINDOW_MS {
                    self.start_hug(at);
                    return;
                }
            }
        }

        let _ = self.try_start_reaction(
            Expression::Happy,
            ReactionReason::KeyLeft,
            PRIO_PHYSICAL,
            at,
            KEY_HAPPY_MS,
            Some(HapticPattern::Happy),
        );
    }
    pub(super) fn on_key_right_down(&mut self, at: u64) {
        if self.key_right_down {
            return;
        }
        self.key_right_down = true;
        self.key_right_down_at = Some(at);

        if self.key_left_down {
            if let Some(left_at) = self.key_left_down_at {
                if at.saturating_sub(left_at) <= HUG_WINDOW_MS {
                    self.start_hug(at);
                    return;
                }
            }
        }

        push_window(
            &mut self.r_key_times,
            &mut self.r_key_count,
            at,
            KEY_TRIPLE_WINDOW_MS,
        );
        if self.r_key_count >= KEY_TRIPLE_COUNT {
            self.r_key_count = 0;
            if at >= self.angry_cooldown_until
                && !self.is_panic_active(at)
                && !self.is_angry_active(at)
            {
                self.start_angry(at, ReactionReason::KeyRight);
                return;
            }
        }

        let _ = self.try_start_reaction(
            Expression::Puzzled,
            ReactionReason::KeyRight,
            PRIO_PHYSICAL,
            at,
            KEY_PUZZLED_MS,
            Some(HapticPattern::Puzzled),
        );
    }
    pub(super) fn start_hug(&mut self, at: u64) {
        self.r_key_count = 0;
        self.poke_count = 0;
        let _ = self.try_start_reaction(
            Expression::Satisfied,
            ReactionReason::BothKeys,
            PRIO_PHYSICAL,
            at,
            HUG_MS,
            Some(HapticPattern::Satisfied),
        );
    }
    pub(super) fn on_button_short(&mut self, at: u64, source: EventSource) {
        if source != EventSource::ButtonPower {
            return;
        }
        // Listening starts either way; the Activity layer hides the face.
        self.keep_awake(at);
        if !self.error_active {
            self.activity = Some(Expression::Listening);
            self.fallback_since_ms = at;
        }
        self.pending_command = Some(SystemCommand::StartListening);
        self.queue_haptic(HapticPattern::Listening);
    }
    pub(super) fn on_button_long(&mut self, _at: u64, source: EventSource) {
        if source != EventSource::ButtonPower {
            return;
        }
        // No expression change and no pattern exists for this generic tick in
        // the frozen HapticPattern set.
        self.pending_command = Some(SystemCommand::Hub);
    }
    pub(super) fn on_button_double(&mut self, at: u64, source: EventSource) {
        if source != EventSource::ButtonPower {
            return;
        }
        self.activity = None;
        self.fallback_since_ms = at;
        self.pending_command = Some(SystemCommand::StopActivity);
    }
    pub(super) fn extend_key_hold(&mut self, now: u64) {
        if !self.key_left_down {
            return;
        }
        if let Some(reaction) = &mut self.reaction {
            if reaction.reason == ReactionReason::KeyLeft
                && reaction.expression == Expression::Happy
                && reaction.priority == PRIO_PHYSICAL
            {
                reaction.ends_ms = now.saturating_add(KEY_HAPPY_MS);
            }
        }
    }
}
