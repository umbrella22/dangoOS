//! Light, dawn, night and debounced presence reactions.
use super::*;

impl AffectEngine {
    pub(super) fn advance_pir(&mut self, now: u64) {
        if self.pir_raw != self.pir_debounced {
            let confirm_at = self.pir_raw_since.saturating_add(PIR_DEBOUNCE_MS);
            if now >= confirm_at {
                let rising = self.pir_raw;
                self.pir_debounced = rising;
                if rising {
                    let same_visit = match self.pir_absence_since {
                        Some(fell) => confirm_at < fell.saturating_add(PIR_VISIT_GAP_MS),
                        None => self.pir_presence_since.is_some(),
                    };
                    self.pir_rise_ms = confirm_at;
                    self.pir_absence_since = None;
                    self.wake(confirm_at);
                    if !same_visit {
                        self.pir_presence_since = Some(confirm_at);
                        self.pir_sustained_fired = false;
                        self.fallback_since_ms = confirm_at;
                        let _ = self.try_start_reaction(
                            Expression::Curious,
                            ReactionReason::Presence,
                            PRIO_AMBIENT,
                            confirm_at,
                            PIR_CURIOUS_MS,
                            None,
                        );
                    }
                } else {
                    // Keep `pir_presence_since`: a quick return continues the visit.
                    self.pir_absence_since = Some(confirm_at);
                }
            }
        }

        if self.pir_debounced {
            if let Some(since) = self.pir_presence_since {
                // The visit may have had short lows; fire on the first high
                // sample at or after the ten-second mark.
                let fire_at = since.saturating_add(PIR_SUSTAINED_MS).max(self.pir_rise_ms);
                if now >= fire_at && !self.pir_sustained_fired {
                    let allowed = match self.pir_last_satisfied_ms {
                        Some(last) => fire_at >= last.saturating_add(PIR_SATISFIED_COOLDOWN_MS),
                        None => true,
                    };
                    if allowed
                        && self.try_start_reaction(
                            Expression::Satisfied,
                            ReactionReason::Presence,
                            PRIO_AMBIENT,
                            fire_at,
                            PIR_SATISFIED_MS,
                            Some(HapticPattern::Satisfied),
                        )
                    {
                        self.pir_last_satisfied_ms = Some(fire_at);
                    }
                    self.pir_sustained_fired = true;
                }
            }
        }

        if let Some(since) = self.pir_absence_since {
            if now >= since.saturating_add(PIR_VISIT_GAP_MS) {
                self.pir_presence_since = None;
                self.pir_sustained_fired = false;
            }
            let sleep_at = since
                .saturating_add(PIR_ABSENCE_SLEEP_MS)
                .max(self.awake_until.unwrap_or(0));
            if !self.pir_debounced && now >= sleep_at && self.idle_expression != Expression::Sleep {
                self.idle_expression = Expression::Sleep;
                self.fallback_since_ms = sleep_at;
            }
        }
    }
    pub(super) fn advance_dawn(&mut self, now: u64) {
        let level = match self.light_level {
            Some(level) => level,
            None => return,
        };
        let reference_at = match self.dawn_ref_at {
            Some(reference_at) => reference_at,
            None => return,
        };
        if now < reference_at.saturating_add(DAWN_WINDOW_MS) {
            return;
        }

        if level >= self.dawn_ref_level.saturating_add(DAWN_RISE) {
            let _ = self.try_start_reaction(
                Expression::Curious,
                ReactionReason::Light,
                PRIO_AMBIENT,
                now,
                DAWN_CURIOUS_MS,
                None,
            );
            self.dawn_ref_level = level;
            self.dawn_ref_at = Some(now);
        } else if level < self.dawn_ref_level {
            self.dawn_ref_level = level;
            self.dawn_ref_at = Some(now);
        }
    }
    pub(super) fn advance_night(&mut self, now: u64) {
        if let Some(since) = self.dark_since {
            let sleep_at = since
                .saturating_add(NIGHT_SLEEP_MS)
                .max(self.awake_until.unwrap_or(0));
            if now >= sleep_at && self.idle_expression != Expression::Sleep {
                self.idle_expression = Expression::Sleep;
                self.fallback_since_ms = sleep_at;
            }
        }
    }
    pub(super) fn on_light_level(&mut self, at: u64, level: u8) {
        let level = level.min(100);
        self.light_history.push(LightSample { level, at_ms: at });

        if level >= LIGHT_WAKE_LEVEL {
            self.wake(at);
            self.dark_since = None;
        } else if level >= LIGHT_DARK_LEVEL {
            self.dark_since = None;
        } else if self.dark_since.is_none() {
            self.dark_since = Some(at);
        }

        if let Some(oldest) = self.light_history.oldest_within(at, LIGHT_FAST_WINDOW_MS) {
            if oldest.at_ms < at {
                let before = oldest.level as i32;
                let now = level as i32;
                let threshold = LIGHT_FAST_MIN.max(before.max(now) * LIGHT_FAST_PCT / 100);
                if now - before >= threshold {
                    // Light floods back: she squeezes her eyes shut.
                    self.light_cover = None;
                    let _ = self.try_start_reaction(
                        Expression::Dazzled,
                        ReactionReason::Light,
                        PRIO_AMBIENT,
                        at,
                        LIGHT_BRIGHT_MS,
                        Some(HapticPattern::Surprised),
                    );
                    self.light_history
                        .restart_from(LightSample { level, at_ms: at });
                    self.reanchor_dawn(at, level);
                } else if before - now >= threshold && self.light_cover.is_none() {
                    // Covered: wide eyes, straining to see.
                    self.light_cover = Some(LightCover {
                        before: oldest.level,
                        since_ms: at,
                    });
                    let _ = self.try_start_reaction(
                        Expression::Peering,
                        ReactionReason::Light,
                        PRIO_AMBIENT,
                        at,
                        LIGHT_DARK_MS,
                        None,
                    );
                    self.light_history
                        .restart_from(LightSample { level, at_ms: at });
                    self.reanchor_dawn(at, level);
                }
            }
        }
        self.hold_cover(at, level);

        if self.dawn_ref_at.is_none() {
            self.reanchor_dawn(at, level);
        }
        self.light_level = Some(level);
    }
    /// Keep peering while the sensor stays covered.
    pub(super) fn hold_cover(&mut self, at: u64, level: u8) {
        let Some(cover) = self.light_cover else {
            return;
        };
        let before = cover.before as i32;
        let threshold = LIGHT_FAST_MIN.max(before * LIGHT_FAST_PCT / 100);
        if (level as i32) > before - threshold / 2 {
            // The light came back slowly: no flash, just the end of the cover.
            self.light_cover = None;
            return;
        }
        if let Some(reaction) = &mut self.reaction {
            if reaction.expression == Expression::Peering && at < reaction.ends_ms {
                let cap = cover.since_ms.saturating_add(LIGHT_COVER_MAX_MS);
                reaction.ends_ms = at
                    .saturating_add(LIGHT_DARK_MS)
                    .min(cap)
                    .max(reaction.ends_ms);
            }
        }
    }
    pub(super) fn reanchor_dawn(&mut self, at: u64, level: u8) {
        self.dawn_ref_level = level;
        self.dawn_ref_at = Some(at);
    }
    pub(super) fn on_presence(&mut self, at: u64, present: bool) {
        if present == self.pir_raw {
            return;
        }
        self.pir_raw = present;
        self.pir_raw_since = at;
    }
}
