//! IMU motion accumulation, violent peaks and burst history.
use super::*;

impl AffectEngine {
    pub(super) fn on_accel(&mut self, at: u64, ax_mg: i16, ay_mg: i16, az_mg: i16) {
        let dynamic_mg = accel_dynamic_mg(ax_mg, ay_mg, az_mg);

        if dynamic_mg < ACCEL_REST_MG {
            self.shake.burst_above_small = false;
            self.shake.violent_above = false;
            self.drain_dizzy(at);
            return;
        }

        self.note_motion(at);
        let how = if dynamic_mg >= ACCEL_VIOLENT_MG {
            Rouse::Rough
        } else {
            Rouse::Gentle
        };
        let _ = self.rouse(at, how);

        if dynamic_mg > ACCEL_LARGE_MG {
            self.shake.burst_large = true;
        }
        if dynamic_mg >= ACCEL_SMALL_MG {
            if !self.shake.burst_above_small {
                self.shake.burst_above_small = true;
                self.shake.burst_small_peaks = self.shake.burst_small_peaks.saturating_add(1);
            }
            self.add_dizzy(at, dynamic_mg.min(DIZZY_ACCEL_CAP_MG) as u32 / 10);
        } else {
            self.shake.burst_above_small = false;
            self.drain_dizzy(at);
        }

        if dynamic_mg >= ACCEL_VIOLENT_MG {
            if !self.shake.violent_above {
                self.shake.violent_above = true;
                self.on_violent_peak(at);
            }
        } else if dynamic_mg < ACCEL_VIOLENT_RELEASE_MG {
            self.shake.violent_above = false;
        }
    }
    pub(super) fn on_gyro(&mut self, at: u64, gx_dps: i16, gy_dps: i16, gz_dps: i16) {
        let max_dps = gyro_max_dps(gx_dps, gy_dps, gz_dps);
        if max_dps < GYRO_REST_DPS {
            self.drain_dizzy(at);
            return;
        }
        self.note_motion(at);
        let _ = self.rouse(at, Rouse::Gentle);
        // Turning the fox over in the hand is not a reason to scream: a fast
        // rotation only makes her dizzier.
        if max_dps >= GYRO_SHAKY_DPS {
            self.add_dizzy(at, max_dps.min(DIZZY_GYRO_CAP_DPS) as u32 / 10);
        } else {
            self.drain_dizzy(at);
        }
    }
    /// Leak the dizzy bucket up to `at`.
    pub(super) fn drain_dizzy(&mut self, at: u64) {
        let elapsed = at.saturating_sub(self.shake.dizzy_at_ms);
        let leak = (elapsed.min(60_000) as u32).saturating_mul(DIZZY_LEAK_PER_S) / 1_000;
        self.shake.dizzy = self.shake.dizzy.saturating_sub(leak);
        self.shake.dizzy_at_ms = self.shake.dizzy_at_ms.max(at);
    }
    /// Pour one shaky sample into the dizzy bucket; full means dizzy.
    pub(super) fn add_dizzy(&mut self, at: u64, gain: u32) {
        self.drain_dizzy(at);
        self.shake.dizzy = (self.shake.dizzy + gain).min(DIZZY_MAX);
        if self.shake.dizzy >= DIZZY_TRIGGER {
            self.on_dizzy(at);
        }
    }
    pub(super) fn on_violent_peak(&mut self, at: u64) {
        let shake = &mut self.shake;
        let len = shake.violent_len as usize;
        if len == VIOLENT_PEAKS {
            shake.violent_peaks.copy_within(1.., 0);
            shake.violent_peaks[VIOLENT_PEAKS - 1] = at;
        } else {
            shake.violent_peaks[len] = at;
            shake.violent_len += 1;
        }
        if shake.violent_len as usize == VIOLENT_PEAKS
            && at.saturating_sub(shake.violent_peaks[0]) <= VIOLENT_WINDOW_MS
        {
            shake.violent_len = 0;
            if !self.is_panic_active(at) {
                self.shake.burst_confused_emitted = true;
                self.trigger_panic(at);
            }
        }
    }
    pub(super) fn note_motion(&mut self, at: u64) {
        if !self.shake.burst_active {
            self.shake.burst_active = true;
            self.shake.burst_first_motion_ms = at;
            self.shake.burst_above_small = false;
            self.shake.burst_small_peaks = 0;
            self.shake.burst_large = false;
            self.shake.burst_confused_emitted = false;
        }
        self.shake.burst_last_motion_ms = at;

        // "Keep current Confused" while motion remains above rest.
        if let Some(reaction) = &mut self.reaction {
            if reaction.reason == ReactionReason::ShakeSmall
                && reaction.expression == Expression::Confused
                && at < reaction.ends_ms
            {
                reaction.ends_ms = at.saturating_add(DIZZY_MS);
            }
        }
    }
    pub(super) fn on_dizzy(&mut self, at: u64) {
        if let Some(reaction) = self.reaction {
            if reaction.reason == ReactionReason::ShakeSmall
                && reaction.expression == Expression::Confused
                && at < reaction.ends_ms
            {
                if let Some(active) = &mut self.reaction {
                    active.ends_ms = at.saturating_add(DIZZY_MS);
                }
                self.shake.burst_confused_emitted = true;
                return;
            }
            if reaction.priority >= PRIO_PHYSICAL && at < reaction.ends_ms {
                return;
            }
        }

        if self.module_sequence.is_some() {
            return;
        }

        let haptic = if self.shake.burst_confused_emitted {
            None
        } else {
            Some(HapticPattern::Confused)
        };
        if self.try_start_reaction(
            Expression::Confused,
            ReactionReason::ShakeSmall,
            PRIO_PHYSICAL,
            at,
            DIZZY_MS,
            haptic,
        ) {
            self.shake.burst_confused_emitted = true;
        }
    }
    pub(super) fn update_burst(&mut self, now: u64) {
        if !self.shake.burst_active {
            return;
        }
        let close_at = self
            .shake
            .burst_last_motion_ms
            .saturating_add(REST_CONFIRM_MS);
        if now < close_at {
            return;
        }

        self.shake.burst_active = false;
        let had_small = self.shake.burst_small_peaks >= 1;
        let was_large = self.shake.burst_large;
        let first = self.shake.burst_first_motion_ms;
        let last = self.shake.burst_last_motion_ms;

        self.shake.burst_above_small = false;
        self.shake.burst_small_peaks = 0;
        self.shake.burst_large = false;
        self.shake.burst_confused_emitted = false;

        if was_large || !had_small {
            return;
        }

        let gap_valid = match self.shake.prev_burst_last_motion_ms {
            Some(previous) => {
                let gap = first.saturating_sub(previous);
                (BURST_GAP_MIN_MS..=BURST_GAP_MAX_MS).contains(&gap)
            }
            None => false,
        };
        let window_valid = self.shake.small_burst_count > 0
            && first.saturating_sub(self.shake.burst_window_start_ms) <= BURST_WINDOW_MS;

        if gap_valid && window_valid {
            self.shake.small_burst_count = self.shake.small_burst_count.saturating_add(1);
        } else {
            self.shake.small_burst_count = 1;
            self.shake.burst_window_start_ms = first;
        }

        self.shake.prev_burst_last_motion_ms = Some(last);

        if self.shake.small_burst_count >= ANGRY_BURST_COUNT {
            self.shake.small_burst_count = 0;
            self.shake.burst_window_start_ms = close_at;
            if close_at >= self.angry_cooldown_until
                && !self.is_panic_active(close_at)
                && !self.is_angry_active(close_at)
            {
                self.start_angry(close_at, ReactionReason::ShakeBursts);
            }
        }
    }
}

fn isqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}
fn accel_dynamic_mg(ax_mg: i16, ay_mg: i16, az_mg: i16) -> i32 {
    let ax = ax_mg as i64;
    let ay = ay_mg as i64;
    let az = az_mg as i64;
    let squared = ax * ax + ay * ay + az * az;
    let magnitude = isqrt(squared as u64) as i64;
    let dynamic = magnitude - 1_000;
    if dynamic < 0 {
        (-dynamic) as i32
    } else {
        dynamic as i32
    }
}
fn gyro_max_dps(gx_dps: i16, gy_dps: i16, gz_dps: i16) -> i32 {
    let gx = (gx_dps as i32).abs();
    let gy = (gy_dps as i32).abs();
    let gz = (gz_dps as i32).abs();
    gx.max(gy).max(gz)
}
