//! Sensor affect engine (H0).
//!
//! `SensorEvent` enters here; `AffectEngine` selects a
//! [`crate::companion::behavior::Expression`] and a haptic pattern. The engine is pure
//! Rust: no hardware access, no LVGL, no RTOS, no wall clock, no allocation.
//!
//! ## Determinism and tick semantics
//!
//! The engine only advances from caller-supplied millisecond timestamps. The
//! same event stream and the same `tick` times always produce the same state.
//! Time is expected to be monotonic; an event or tick timestamp older than the
//! last processed time is clamped to the last processed time.
//!
//! `tick(now)` advances elapsed timers and resolves the expression. Calling
//! `tick` twice with the same `now` leaves the resolved [`AffectState`]
//! identical and reports `changed == false` on the second call. Haptic and
//! command fields are one-shot outputs: the first tick consumes them, which is
//! the only intentional same-timestamp difference.
//!
//! ## Thresholds
//!
//! Values marked "plan" come from RFC 0004 section 4. Values marked "chosen"
//! fill a gap in the plan. RFC 0004 (and RFC 0005, cited by the tasks) are no
//! longer in `docs/`; read them with
//! `git show 4f601de:docs/plans/grok-ball-sensor-reactions-plan.md` (or
//! `platform-runtime-plan.md`). This table is the current specification.
//!
//! | Item | Value |
//! | --- | --- |
//! | Rest | dynamic accel `< 80 mg` and gyro `< 20 dps` (plan) |
//! | Shaky sample | dynamic accel `>= 300 mg` or gyro `>= 150 dps` |
//! | Dizzy | shaky samples fill a leaky bucket (`+min(mg,1000)/10`, `+min(dps,500)/10`, leak `500/s`); `>= 1500` → Confused (chosen: a pickup stays far below) |
//! | Violent shake | `5` rising peaks of dynamic accel `>= 950 mg` within `1 s` → Panic (chosen; the plan's two peaks or one gyro spike fired on every pickup) |
//! | Burst rest | `300 ms` below rest (plan) |
//! | Burst gap | `300..=800 ms` (plan) |
//! | Angry bursts | `5` within `15 s` (plan 5–10; 5 chosen, see tests) |
//! | Panic hold / still / confused | `1.5 s` / `5 s` / `3 s` (plan) |
//! | Dizzy confused | `2.5 s` after the last motion (plan) |
//! | Angry hold / cooldown | `10 s` / `30 s` after the hold (plan) |
//! | Petting hold | `1.5 s` after the last pet (plan 4.2) |
//! | Poke | `1.5 s`; three pokes within `1.5 s` → Angry (plan) |
//! | Key L / R | Happy `1.5 s` while held; Puzzled `1.5 s` |
//! | Key R triple | `3` within `1.5 s` (plan 4.5) |
//! | Hug window / hold | `300 ms` / `2 s` (plan) |
//! | Light cover | drop of `max(8, 40% of before)` inside `400 ms` → Peering, held while covered (`<= 15 s`) |
//! | Light flash | rise of `max(8, 40% of after)` inside `400 ms` → Dazzled `3 s` (relative, so a dim room works) |
//! | Light dawn | rise of `20` points over `30 s` (plan concept, chosen rise) |
//! | Night | level `< 10` for `60 s`; wake at level `>= 20` (chosen wake level) |
//! | PIR debounce | `200 ms` (plan) |
//! | PIR sustained / cooldown | `10 s` / `30 s` (plan) |
//! | PIR visit | lows shorter than `30 s` join one visit: one Curious greeting per visit (the sensor pulses every few seconds) |
//! | PIR absence | `60 s` → Sleep (plan) |
//! | BMM150 confidence | `>= 50` accepted (plan `>= 0.5`) |
//! | Haptic rate limit | one pattern per `150 ms` (plan) |
//! | Battery tired | state of charge `<= 15%`; leaves above `18%` (chosen) |
//! | Battery dying | state of charge `<= 5%`; leaves above `8%` (chosen) |
//! | Awake hold | `5 min` after the last interaction no sleep timer may fire (chosen; the user asked for 5–10 min) |
//! | Wake face | drowsy (`Tired`) `3 s`; grumpy (`Angry`) `2.5 s` for a knock `>= 950 mg` or a second poke/tap while drowsy |
//!
//! The H0 event contract has no pickup/flip or long-stroke event and the H0
//! work package does not include those rules; they are not implemented here.
//!
//! ## Priority, Activity and Error
//!
//! The rendering priority is
//! `Error > Panic > Angry (during its 10 s hold) > physical > ambient >
//! Activity > Idle`. Activity is a persistent base layer, not a high-priority
//! transient: short physical and ambient reactions render above it for their
//! brief lifetime, and Activity returns when they expire.
//!
//! `EventKind::Activity(Expression)` is the external voice/LLM input:
//!
//! - `Listening`, `Thinking` and `Speaking` set the Activity layer.
//! - `Error` is the highest layer. It clears any Panic, Angry, module,
//!   physical or ambient reaction and persists until `Activity(Idle)`
//!   arrives. `Activity(Idle)` is the only event that clears Error. Other
//!   Activity values are ignored while Error is active.
//! - `Idle` clears a normal Activity layer as well as Error.
//! - Any other expression is ignored.
//!
//! Angry interrupts non-Error Activity for its hold and resumes it when the
//! hold expires. Panic also interrupts non-Error Activity: its complete
//! sequence (Panic hold, still wait, post-panic Confused) masks Activity, and
//! Activity resumes when the sequence ends.
//!
//! The plan defines no numeric Mood rule, so the idle baseline stands in for
//! Mood.
//!
//! ## Battery baseline
//!
//! `EventKind::BatteryLevel` feeds the BQ27220 state of charge. It is not a
//! reaction: a low battery changes the resting face, so `Tired`/`Dying` replace
//! the idle baseline while reactions and Activity still play above them.
//! Thresholds have 3-point hysteresis so a gauge flickering around 15% cannot
//! make the face flap between `Idle` and `Tired`. Because it replaces the
//! resting layer, a low battery also shows through Sleep: she wakes up tired.
//!
//! ## Sleep and waking
//!
//! She falls asleep after `60 s` without presence (PIR) or `60 s` of darkness,
//! which also happens once the Interaction board, and with it the PIR and the
//! light sensor, is gone. Any interaction wakes her: screen taps
//! ([`AffectEngine::on_screen_touch`]), board touch and keys, the power
//! button, picking her up or bumping her, a magnet, a module plugged in.
//! Each interaction holds the sleep timers off for the awake hold; when it
//! runs out and the room is still empty or dark, she goes back to sleep.
//!
//! Being woken shows on her face. A gentle interaction (a stroke, a key, a
//! tap, lifting her) gets a drowsy face; a knock (`>= 950 mg`) or poking her
//! again while she is still drowsy makes her grumpy. That first interaction
//! is spent on waking up and does not trigger its own reaction.

use crate::companion::behavior::Expression;

mod ambient;
mod interaction;
mod magnetic;
mod modules;
mod motion;
mod rules;
mod types;

use rules::*;
pub(crate) use rules::{
    AWAKE_HOLD_MS, CAMERA_INSERT_CURIOUS_MS, CAMERA_REMOVE_CONFUSED_MS, DAWN_CURIOUS_MS, DIZZY_MS,
    DIZZY_TRIGGER, HUG_MS, INTERACTION_INSERT_CURIOUS_MS, INTERACTION_INSERT_HAPPY_MS,
    INTERACTION_INSERT_SURPRISED_MS, INTERACTION_REMOVE_CONFUSED_MS,
    INTERACTION_REMOVE_SURPRISED_MS, KEY_HAPPY_MS, KEY_PUZZLED_MS, LIGHT_BRIGHT_MS,
    LIGHT_COVER_MAX_MS, LIGHT_DARK_MS, LIGHT_FAST_WINDOW_MS, MIN_DWELL_MS, PIR_CURIOUS_MS,
    PIR_SATISFIED_MS, PIR_VISIT_GAP_MS, VIOLENT_PEAKS, VIOLENT_WINDOW_MS, WAKE_DROWSY_MS,
    WAKE_GRUMPY_MS,
};
pub use types::*;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Reaction {
    expression: Expression,
    reason: ReactionReason,
    priority: u8,
    started_ms: u64,
    ends_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct PanicFollowup {
    panic_end_ms: u64,
    confused_started: bool,
    confused_end_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct ModuleStep {
    expression: Expression,
    reason: ReactionReason,
    duration_ms: u64,
}

impl ModuleStep {
    const EMPTY: Self = Self {
        expression: Expression::Idle,
        reason: ReactionReason::ModuleInsert,
        duration_ms: 0,
    };
}

#[derive(Clone, Copy, Debug)]
struct ModuleSequence {
    steps: [ModuleStep; 3],
    len: u8,
    index: u8,
    stage_started_ms: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct ShakeState {
    /// Rising edges above [`ACCEL_VIOLENT_MG`], oldest first.
    violent_peaks: [u64; VIOLENT_PEAKS],
    violent_len: u8,
    /// The current sample run is above the violent threshold.
    violent_above: bool,
    /// Leaky dizziness bucket and the time it was last drained.
    dizzy: u32,
    dizzy_at_ms: u64,
    burst_active: bool,
    burst_first_motion_ms: u64,
    burst_last_motion_ms: u64,
    burst_above_small: bool,
    burst_small_peaks: u8,
    burst_large: bool,
    burst_confused_emitted: bool,
    small_burst_count: u8,
    burst_window_start_ms: u64,
    prev_burst_last_motion_ms: Option<u64>,
}

/// The sensor was covered: the level before the drop and when it fell.
#[derive(Clone, Copy, Debug)]
struct LightCover {
    before: u8,
    since_ms: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct LightSample {
    level: u8,
    at_ms: u64,
}

#[derive(Clone, Copy, Debug)]
struct LightHistory {
    samples: [LightSample; LIGHT_HISTORY],
    len: u8,
    next: u8,
}

impl LightHistory {
    fn new() -> Self {
        Self {
            samples: [LightSample::default(); LIGHT_HISTORY],
            len: 0,
            next: 0,
        }
    }

    fn push(&mut self, sample: LightSample) {
        self.samples[self.next as usize] = sample;
        self.next = (self.next + 1) % LIGHT_HISTORY as u8;
        if self.len < LIGHT_HISTORY as u8 {
            self.len += 1;
        }
    }

    /// Forget everything but `sample`: a change that already fired a
    /// reaction must not fire it again on every following sample.
    fn restart_from(&mut self, sample: LightSample) {
        self.len = 0;
        self.next = 0;
        self.push(sample);
    }

    /// Oldest sample no more than `window_ms` before `at_ms`, in chronological
    /// order. The sample just pushed is included.
    fn oldest_within(&self, at_ms: u64, window_ms: u64) -> Option<LightSample> {
        let len = self.len as usize;
        let mut i = 0;
        while i < len {
            let index = if self.len < LIGHT_HISTORY as u8 {
                i
            } else {
                (self.next as usize + i) % LIGHT_HISTORY
            };
            let sample = self.samples[index];
            if at_ms.saturating_sub(sample.at_ms) <= window_ms {
                return Some(sample);
            }
            i += 1;
        }
        None
    }
}

/// The deterministic affect engine.
pub struct AffectEngine {
    now_ms: u64,
    last_expression: Expression,
    fallback_since_ms: u64,
    idle_expression: Expression,
    activity: Option<Expression>,
    business: crate::companion::behavior::business::State,
    error_active: bool,
    error_since_ms: u64,
    reaction: Option<Reaction>,
    panic_followup: Option<PanicFollowup>,
    module_sequence: Option<ModuleSequence>,
    pending_haptic: Option<HapticPattern>,
    last_haptic_ms: Option<u64>,
    pending_command: Option<SystemCommand>,
    shake: ShakeState,
    angry_cooldown_until: u64,
    poke_times: [u64; 3],
    poke_count: u8,
    key_left_down: bool,
    key_right_down: bool,
    key_left_down_at: Option<u64>,
    key_right_down_at: Option<u64>,
    r_key_times: [u64; 3],
    r_key_count: u8,
    light_history: LightHistory,
    light_level: Option<u8>,
    light_cover: Option<LightCover>,
    dawn_ref_level: u8,
    dawn_ref_at: Option<u64>,
    dark_since: Option<u64>,
    pir_raw: bool,
    pir_raw_since: u64,
    pir_debounced: bool,
    pir_presence_since: Option<u64>,
    pir_sustained_fired: bool,
    pir_last_satisfied_ms: Option<u64>,
    pir_absence_since: Option<u64>,
    pir_rise_ms: u64,
    magnetic_contacted: bool,
    module_right_present: bool,
    module_left_present: bool,
    /// State-of-charge driven resting face; `None` is the normal idle.
    battery_expression: Option<Expression>,
    /// Sleep timers may not fire before this (last interaction + hold).
    awake_until: Option<u64>,
}

impl AffectEngine {
    /// Create an engine whose first idle timestamp is `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms,
            last_expression: Expression::Idle,
            fallback_since_ms: now_ms,
            idle_expression: Expression::Idle,
            activity: None,
            business: crate::companion::behavior::business::State::default(),
            error_active: false,
            error_since_ms: 0,
            reaction: None,
            panic_followup: None,
            module_sequence: None,
            pending_haptic: None,
            last_haptic_ms: None,
            pending_command: None,
            shake: ShakeState::default(),
            angry_cooldown_until: 0,
            poke_times: [0; 3],
            poke_count: 0,
            key_left_down: false,
            key_right_down: false,
            key_left_down_at: None,
            key_right_down_at: None,
            r_key_times: [0; 3],
            r_key_count: 0,
            light_history: LightHistory::new(),
            light_level: None,
            light_cover: None,
            dawn_ref_level: 0,
            dawn_ref_at: None,
            dark_since: None,
            pir_raw: false,
            pir_raw_since: now_ms,
            pir_debounced: false,
            pir_presence_since: None,
            pir_sustained_fired: false,
            pir_last_satisfied_ms: None,
            pir_absence_since: None,
            pir_rise_ms: 0,
            magnetic_contacted: false,
            module_right_present: false,
            module_left_present: false,
            battery_expression: None,
            awake_until: None,
        }
    }

    /// A press on the screen (the fox page). Returns `true` when the press
    /// was spent on waking her up, so the caller should not also start a tap
    /// reaction for it.
    pub fn on_screen_touch(&mut self, at_ms: u64) -> bool {
        let at = at_ms.max(self.now_ms);
        self.advance(at);
        self.rouse(at, Rouse::Poke)
    }

    /// Interaction that does not involve the fox page (the hub apps): keeps
    /// her awake without a wake face.
    pub fn on_interaction(&mut self, at_ms: u64) {
        let at = at_ms.max(self.now_ms);
        self.advance(at);
        self.keep_awake(at);
    }

    /// Whether the resting layer is Sleep (ignoring the battery face).
    pub fn is_asleep(&self) -> bool {
        self.idle_expression == Expression::Sleep
    }

    /// Apply one sensor event. Timers are advanced to the event timestamp
    /// before the event itself is processed.
    pub fn on_event(&mut self, event: SensorEvent) {
        let at = event.at_ms.max(self.now_ms);
        self.advance(at);

        match event.kind {
            EventKind::Accel {
                ax_mg,
                ay_mg,
                az_mg,
            } => self.on_accel(at, ax_mg, ay_mg, az_mg),
            EventKind::Gyro {
                gx_dps,
                gy_dps,
                gz_dps,
            } => self.on_gyro(at, gx_dps, gy_dps, gz_dps),
            EventKind::TouchPet => self.on_touch_pet(at),
            EventKind::TouchPoke => self.on_touch_poke(at),
            EventKind::LightLevel(level) => self.on_light_level(at, level),
            EventKind::Presence(present) => self.on_presence(at, present),
            EventKind::KeyDown => self.on_key_down(at, event.source),
            EventKind::KeyUp => self.on_key_up(at, event.source),
            EventKind::ButtonShort => self.on_button_short(at, event.source),
            EventKind::ButtonLong => self.on_button_long(at, event.source),
            EventKind::ButtonDouble => self.on_button_double(at, event.source),
            EventKind::MagneticApproach => self.on_magnetic_approach(at),
            EventKind::MagneticContact => self.on_magnetic_contact(at),
            EventKind::MagneticRelease => self.on_magnetic_release(at),
            EventKind::MagneticOrbit => self.on_magnetic_orbit(at),
            EventKind::MagneticEdge { confidence } => self.on_magnetic_edge(at, confidence),
            EventKind::ModuleInserted(kind) => self.on_module_inserted(at, event.source, kind),
            EventKind::ModuleRemoved(kind) => self.on_module_removed(at, event.source, kind),
            EventKind::BatteryLevel(percent) => self.on_battery_level(at, percent),
            EventKind::Activity(expression) => self.on_activity(at, expression),
            kind @ (EventKind::NetworkConnection(_)
            | EventKind::BluetoothConnection(_)
            | EventKind::MusicPlayback(_)) => self.business.observe(at, kind),
        }
    }

    /// Advance timers to `now_ms` and return the resolved expression.
    pub fn tick(&mut self, now_ms: u64) -> AffectTick {
        let now = now_ms.max(self.now_ms);
        self.advance(now);

        let (expression, reason, since_ms) = self.resolve();
        let changed = expression != self.last_expression;
        self.last_expression = expression;

        let haptic = self.take_haptic(now);
        let command = self.pending_command.take();

        AffectTick {
            state: AffectState {
                expression,
                intensity: expression_intensity(expression),
                since_ms,
                reason,
            },
            changed,
            haptic,
            command,
        }
    }

    /// The resolved state without advancing time, consuming nothing.
    ///
    /// Used by the runtime arbiter to read the affect layer after an event was
    /// applied but before the next `tick`.
    pub fn snapshot(&self) -> AffectState {
        let (expression, reason, since_ms) = self.resolve();
        AffectState {
            expression,
            intensity: expression_intensity(expression),
            since_ms,
            reason,
        }
    }

    /// The state the engine would resolve at `now_ms` if the timers advanced,
    /// without mutating anything.
    ///
    /// Reaction and connection expiry are evaluated at the supplied time;
    /// ambient timers advance on the next real `tick`.
    pub fn snapshot_at(&self, now_ms: u64) -> AffectState {
        let now = now_ms.max(self.now_ms);
        let skip_reaction = self
            .reaction
            .is_some_and(|reaction| now >= reaction.ends_ms);
        let (expression, reason, since_ms) = self.resolve_at(now, skip_reaction);
        AffectState {
            expression,
            intensity: expression_intensity(expression),
            since_ms,
            reason,
        }
    }

    // -----------------------------------------------------------------------
    // Timer advancement
    // -----------------------------------------------------------------------

    fn advance(&mut self, now: u64) {
        let now = now.max(self.now_ms);
        self.now_ms = now;

        // A held KEY_L keeps Happy alive. Do this before expiring reactions.
        self.extend_key_hold(now);

        self.update_burst(now);
        self.expire_reaction(now);
        self.advance_panic(now);
        self.expire_reaction(now);
        self.advance_module_sequence(now);
        self.advance_pir(now);
        self.advance_dawn(now);
        self.advance_night(now);

        // A timer may have started a reaction whose window already closed.
        self.expire_reaction(now);
    }

    fn resolve(&self) -> (Expression, ReactionReason, u64) {
        self.resolve_at(self.now_ms, false)
    }

    fn resolve_at(&self, now_ms: u64, skip_reaction: bool) -> (Expression, ReactionReason, u64) {
        if self.error_active {
            return (
                Expression::Error,
                ReactionReason::Error,
                self.error_since_ms,
            );
        }
        if !skip_reaction {
            if let Some(reaction) = self.reaction {
                return (reaction.expression, reaction.reason, reaction.started_ms);
            }
        }
        if let Some(sequence) = self.module_sequence {
            let step = sequence.steps[sequence.index as usize];
            return (step.expression, step.reason, sequence.stage_started_ms);
        }
        if let Some(expression) = self.activity {
            if !skip_reaction && self.panic_followup.is_some() {
                // Panic's full sequence masks Activity until it ends.
                return (
                    self.idle_expression,
                    ReactionReason::Idle,
                    self.fallback_since_ms,
                );
            }
            return (expression, ReactionReason::Activity, self.fallback_since_ms);
        }
        // The battery face is the resting layer: it replaces Idle/Sleep, but
        // every reaction and Activity above still wins.
        if let Some(expression) = self.battery_expression {
            return (expression, ReactionReason::Battery, self.fallback_since_ms);
        }
        if let Some((expression, since)) = self.business.expression(now_ms) {
            return (expression, ReactionReason::Activity, since);
        }
        (
            self.idle_expression,
            ReactionReason::Idle,
            self.fallback_since_ms,
        )
    }

    fn expire_reaction(&mut self, now: u64) {
        if let Some(reaction) = self.reaction {
            if now >= reaction.ends_ms {
                self.reaction = None;
                self.fallback_since_ms = reaction.ends_ms;
            }
        }
    }

    fn advance_panic(&mut self, now: u64) {
        if self.error_active {
            self.panic_followup = None;
            return;
        }
        let mut followup = match self.panic_followup {
            Some(followup) => followup,
            None => return,
        };
        if now < followup.panic_end_ms {
            return;
        }

        if !followup.confused_started {
            let still_from = self.shake.burst_last_motion_ms.max(followup.panic_end_ms);
            let start = still_from.saturating_add(PANIC_STILL_MS);
            let end = start.saturating_add(PANIC_CONFUSED_MS);
            if now < start {
                return;
            }
            if now >= end {
                self.panic_followup = None;
                self.fallback_since_ms = end;
                return;
            }
            let _ = self.try_start_reaction(
                Expression::Confused,
                ReactionReason::ShakeLarge,
                PRIO_PHYSICAL,
                start,
                PANIC_CONFUSED_MS,
                Some(HapticPattern::Confused),
            );
            followup.confused_started = true;
            followup.confused_end_ms = end;
            self.panic_followup = Some(followup);
            return;
        }

        if now >= followup.confused_end_ms {
            if let Some(reaction) = self.reaction {
                if reaction.expression == Expression::Confused
                    && reaction.reason == ReactionReason::ShakeLarge
                    && reaction.ends_ms == followup.confused_end_ms
                {
                    self.reaction = None;
                }
            }
            self.fallback_since_ms = followup.confused_end_ms;
            self.panic_followup = None;
        }
    }

    fn wake(&mut self, at: u64) {
        self.dark_since = None;
        if self.idle_expression == Expression::Sleep {
            self.idle_expression = Expression::Idle;
            self.fallback_since_ms = at;
        }
    }

    /// Wake up silently and hold the sleep timers off.
    fn keep_awake(&mut self, at: u64) {
        self.awake_until = Some(at.saturating_add(AWAKE_HOLD_MS));
        self.wake(at);
    }

    /// An interaction: wake up, hold the sleep timers off, and show how it
    /// felt to be woken. Returns `true` when the interaction was spent on a
    /// wake face (the caller then skips its own reaction).
    fn rouse(&mut self, at: u64, how: Rouse) -> bool {
        let asleep = self.idle_expression == Expression::Sleep;
        // Sleep must actually be on screen for a wake face to make sense.
        let sleep_visible = asleep
            && !self.error_active
            && self.activity.is_none()
            && self.battery_expression.is_none()
            && self.reaction.is_none()
            && self.module_sequence.is_none();
        let drowsy = matches!(
            self.reaction,
            Some(r) if r.reason == ReactionReason::Wake
                && r.expression == Expression::Tired
                && at < r.ends_ms
        );
        self.keep_awake(at);

        let grumpy = match (sleep_visible, drowsy, how) {
            (true, _, Rouse::Rough) => true,
            (true, _, _) => false,
            (false, true, Rouse::Poke | Rouse::Rough) => true,
            _ => return false,
        };
        if grumpy {
            self.reaction = None;
            self.try_start_reaction(
                Expression::Angry,
                ReactionReason::Wake,
                PRIO_PHYSICAL,
                at,
                WAKE_GRUMPY_MS,
                Some(HapticPattern::Angry),
            )
        } else {
            self.try_start_reaction(
                Expression::Tired,
                ReactionReason::Wake,
                PRIO_PHYSICAL,
                at,
                WAKE_DROWSY_MS,
                None,
            )
        }
    }

    // -----------------------------------------------------------------------
    // Reaction selection
    // -----------------------------------------------------------------------

    fn try_start_reaction(
        &mut self,
        expression: Expression,
        reason: ReactionReason,
        priority: u8,
        at: u64,
        duration_ms: u64,
        haptic: Option<HapticPattern>,
    ) -> bool {
        if self.error_active {
            return false;
        }
        if let Some(current) = self.reaction {
            if at < current.ends_ms {
                if priority < current.priority {
                    return false;
                }
                if priority == current.priority && priority >= PRIO_ANGRY {
                    return false;
                }
                if priority == current.priority
                    && reason.is_passive()
                    && reason != current.reason
                    && at < current.started_ms.saturating_add(MIN_DWELL_MS)
                {
                    return false;
                }
            }
        }
        if self.module_sequence.is_some() && priority <= PRIO_PHYSICAL {
            return false;
        }
        if priority >= PRIO_ANGRY {
            self.module_sequence = None;
        }

        self.reaction = Some(Reaction {
            expression,
            reason,
            priority,
            started_ms: at,
            ends_ms: at.saturating_add(duration_ms),
        });
        if let Some(pattern) = haptic {
            self.queue_haptic(pattern);
        }
        true
    }

    fn is_panic_active(&self, at: u64) -> bool {
        match self.reaction {
            Some(reaction) => reaction.priority == PRIO_PANIC && at < reaction.ends_ms,
            None => false,
        }
    }

    fn is_angry_active(&self, at: u64) -> bool {
        match self.reaction {
            Some(reaction) => reaction.priority == PRIO_ANGRY && at < reaction.ends_ms,
            None => false,
        }
    }

    fn trigger_panic(&mut self, at: u64) {
        if self.error_active {
            return;
        }
        self.reaction = Some(Reaction {
            expression: Expression::Panic,
            reason: ReactionReason::ShakeLarge,
            priority: PRIO_PANIC,
            started_ms: at,
            ends_ms: at.saturating_add(PANIC_HOLD_MS),
        });
        self.panic_followup = Some(PanicFollowup {
            panic_end_ms: at.saturating_add(PANIC_HOLD_MS),
            confused_started: false,
            confused_end_ms: 0,
        });
        // Panic interrupts non-Error Activity, but keeps it underneath. The
        // full panic sequence masks the Activity, which resumes when the
        // sequence ends. Activity(Error) is handled before this call.
        self.module_sequence = None;
        self.shake.violent_len = 0;
        self.shake.dizzy = 0;
        self.shake.small_burst_count = 0;
        self.shake.burst_window_start_ms = at;
        self.shake.prev_burst_last_motion_ms = None;
        self.shake.burst_small_peaks = 0;
        self.shake.burst_confused_emitted = true;
        self.queue_haptic(HapticPattern::Panic);
    }

    fn start_angry(&mut self, at: u64, reason: ReactionReason) {
        if self.error_active {
            return;
        }
        self.reaction = Some(Reaction {
            expression: Expression::Angry,
            reason,
            priority: PRIO_ANGRY,
            started_ms: at,
            ends_ms: at.saturating_add(ANGRY_HOLD_MS),
        });
        self.angry_cooldown_until = at
            .saturating_add(ANGRY_HOLD_MS)
            .saturating_add(ANGRY_COOLDOWN_MS);
        self.module_sequence = None;
        self.shake.small_burst_count = 0;
        self.shake.burst_window_start_ms = at;
        self.queue_haptic(HapticPattern::Angry);
    }

    // -----------------------------------------------------------------------
    // External Activity input
    // -----------------------------------------------------------------------

    fn on_activity(&mut self, at: u64, expression: Expression) {
        match expression {
            Expression::Listening | Expression::Thinking | Expression::Speaking => {
                // Error blocks all Activity updates except Activity(Idle).
                if !self.error_active {
                    self.activity = Some(expression);
                    self.fallback_since_ms = at;
                }
            }
            Expression::Error => self.enter_error(at),
            Expression::Idle => {
                self.activity = None;
                if self.error_active {
                    self.error_active = false;
                    self.error_since_ms = 0;
                    if self.pending_haptic == Some(HapticPattern::Error) {
                        self.pending_haptic = None;
                    }
                }
                self.fallback_since_ms = at;
            }
            // Happy and the reaction-only expressions are not Activity inputs.
            _ => {}
        }
    }

    fn enter_error(&mut self, at: u64) {
        if self.error_active {
            return;
        }
        self.error_active = true;
        self.error_since_ms = at;
        self.fallback_since_ms = at;
        // Error interrupts every other layer and is cleared only by
        // Activity(Idle).
        self.activity = None;
        self.reaction = None;
        self.panic_followup = None;
        self.module_sequence = None;
        self.queue_haptic(HapticPattern::Error);
    }

    // -----------------------------------------------------------------------
    // Battery
    // -----------------------------------------------------------------------

    /// Apply a state-of-charge reading from the BQ27220.
    ///
    /// The battery is a *baseline*, not a reaction: it changes what the fox
    /// looks like while resting. Hysteresis keeps a gauge flickering around a
    /// threshold from flapping the face.
    fn on_battery_level(&mut self, at: u64, percent: u8) {
        let percent = percent.min(100);
        let next = match self.battery_expression {
            Some(Expression::Dying) => {
                if percent >= BATTERY_DYING_CLEAR_PCT {
                    if percent <= BATTERY_TIRED_PCT {
                        Some(Expression::Tired)
                    } else {
                        None
                    }
                } else {
                    Some(Expression::Dying)
                }
            }
            Some(Expression::Tired) => {
                if percent >= BATTERY_TIRED_CLEAR_PCT {
                    None
                } else if percent <= BATTERY_DYING_PCT {
                    Some(Expression::Dying)
                } else {
                    Some(Expression::Tired)
                }
            }
            _ => {
                if percent <= BATTERY_DYING_PCT {
                    Some(Expression::Dying)
                } else if percent <= BATTERY_TIRED_PCT {
                    Some(Expression::Tired)
                } else {
                    None
                }
            }
        };

        if next != self.battery_expression {
            self.battery_expression = next;
            self.fallback_since_ms = at;
        }
    }

    // -----------------------------------------------------------------------
    // Haptics
    // -----------------------------------------------------------------------

    fn queue_haptic(&mut self, pattern: HapticPattern) {
        // Error is the top layer; no lower-layer haptic may run while it is
        // active. Error itself replaces any pending lower pattern.
        if self.error_active && pattern != HapticPattern::Error {
            return;
        }
        let priority = pattern.priority();
        match self.pending_haptic {
            None => self.pending_haptic = Some(pattern),
            Some(existing) => {
                if priority >= existing.priority() {
                    self.pending_haptic = Some(pattern);
                }
            }
        }
    }

    fn take_haptic(&mut self, now: u64) -> Option<HapticPattern> {
        let pattern = self.pending_haptic?;
        if let Some(last) = self.last_haptic_ms {
            if now < last.saturating_add(HAPTIC_RATE_LIMIT_MS) {
                return None;
            }
        }
        self.pending_haptic = None;
        self.last_haptic_ms = Some(now);
        Some(pattern)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Intensity mirrors the plan's haptic intensity column where one exists.
/// Idle and Sleep are zero; the remaining values are stable presentation
/// defaults for the renderer.
fn expression_intensity(expression: Expression) -> u8 {
    match expression {
        Expression::Idle | Expression::Sleep => 0,
        Expression::Curious => 40,
        Expression::Happy => 50,
        Expression::Puzzled => 60,
        Expression::Confused => 45,
        Expression::Surprised => 70,
        Expression::Panic => 100,
        Expression::Angry => 90,
        Expression::Satisfied => 55,
        Expression::Listening => 60,
        Expression::Thinking => 35,
        Expression::Speaking => 55,
        Expression::Error => 100,
        Expression::Tired => 25,
        Expression::Dying => 15,
        Expression::Peering => 40,
        Expression::Dazzled => 65,
        Expression::NetworkDisconnected | Expression::BluetoothDisconnected => 35,
        Expression::NetworkConnected | Expression::BluetoothConnected => 50,
        Expression::MusicPlaying => 55,
    }
}

/// Insert `at` into a fixed three-entry sliding window.
fn push_window(times: &mut [u64; 3], count: &mut u8, at: u64, window_ms: u64) {
    if *count >= 3 {
        times[0] = times[1];
        times[1] = times[2];
        times[2] = at;
    } else {
        times[*count as usize] = at;
        *count += 1;
    }

    let len = *count as usize;
    let mut first = 0;
    while first < len && at.saturating_sub(times[first]) > window_ms {
        first += 1;
    }
    if first > 0 {
        let mut index = first;
        while index < len {
            times[index - first] = times[index];
            index += 1;
        }
        *count = (len - first) as u8;
    }
}
