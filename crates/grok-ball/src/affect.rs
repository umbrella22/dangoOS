//! Sensor affect engine (H0).
//!
//! `SensorEvent` enters here; `AffectEngine` selects a
//! [`crate::types::BallExpression`] and a haptic pattern. The engine is pure
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
//! fill a gap in the plan.
//!
//! | Item | Value |
//! | --- | --- |
//! | Rest | dynamic accel `< 80 mg` and gyro `< 20 dps` (plan) |
//! | Small shake | dynamic accel `>= 300 mg`; the unspecified `0.8–1.2 g` band is also small |
//! | Large shake | dynamic accel `> 1200 mg` or gyro `> 200 dps` (plan) |
//! | Large peaks | two within `200 ms` (plan) |
//! | Burst rest | `300 ms` below rest (plan) |
//! | Burst gap | `300..=800 ms` (plan) |
//! | Angry bursts | `5` within `15 s` (plan 5–10; 5 chosen, see tests) |
//! | Panic hold / still / confused | `1.5 s` / `5 s` / `3 s` (plan) |
//! | Small-shake confused | `2.5 s` (plan) |
//! | Angry hold / cooldown | `10 s` / `30 s` after the hold (plan) |
//! | Petting hold | `1.5 s` after the last pet (plan 4.2) |
//! | Poke | `1.5 s`; three pokes within `1.5 s` → Angry (plan) |
//! | Key L / R | Happy `1.5 s` while held; Puzzled `1.5 s` |
//! | Key R triple | `3` within `1.5 s` (plan 4.5) |
//! | Hug window / hold | `300 ms` / `2 s` (plan) |
//! | Light fast delta | `+/-25` points inside `300 ms` (plan), 16-sample window |
//! | Light dawn | rise of `20` points over `30 s` (plan concept, chosen rise) |
//! | Night | level `< 10` for `60 s`; wake at level `>= 20` (chosen wake level) |
//! | PIR debounce | `200 ms` (plan) |
//! | PIR sustained / cooldown | `10 s` / `30 s` (plan) |
//! | PIR absence | `60 s` → Sleep (plan) |
//! | BMM150 confidence | `>= 50` accepted (plan `>= 0.5`) |
//! | Haptic rate limit | one pattern per `150 ms` (plan) |
//! | Battery tired | state of charge `<= 15%`; leaves above `18%` (chosen) |
//! | Battery dying | state of charge `<= 5%`; leaves above `8%` (chosen) |
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
//! `EventKind::Activity(BallExpression)` is the external voice/LLM input:
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
//! make the face flap between `Idle` and `Tired`.

use crate::types::BallExpression;

// ---------------------------------------------------------------------------
// Public event contract
// ---------------------------------------------------------------------------

/// The producer of a [`SensorEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSource {
    Imu,
    Touch,
    Light,
    Pir,
    KeyLeft,
    KeyRight,
    ButtonPower,
    ButtonBoot,
    Bmm150,
    ModuleRight,
    ModuleLeft,
    /// BQ27220 fuel gauge (state of charge).
    Battery,
    /// Voice/LLM/other system layer input; carries [`EventKind::Activity`].
    System,
}

/// The payload of one sensor event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Accelerometer sample in milli-g.
    Accel {
        ax_mg: i16,
        ay_mg: i16,
        az_mg: i16,
    },
    /// Gyroscope sample in degrees per second.
    Gyro {
        gx_dps: i16,
        gy_dps: i16,
        gz_dps: i16,
    },
    /// A petting gesture is in progress.
    TouchPet,
    /// A short poke with little movement.
    TouchPoke,
    /// Filtered relative light level, `0..=100` (not lux).
    LightLevel(u8),
    /// Sampled PIR state. Edges are debounced by the engine.
    Presence(bool),
    KeyDown,
    KeyUp,
    /// `button_power` short press (PTT).
    ButtonShort,
    /// `button_power` long press (hub/drawer).
    ButtonLong,
    /// `button_power` double press (stop/cancel).
    ButtonDouble,
    MagneticApproach,
    /// Edge-triggered: repeated contacts without a release are ignored.
    MagneticContact,
    /// Edge-triggered: ignored unless a contact is active.
    MagneticRelease,
    MagneticOrbit,
    /// Edge change with classifier confidence `0..=100`.
    MagneticEdge {
        confidence: u8,
    },
    /// A module was inserted in the slot named by [`SensorEvent::source`].
    ModuleInserted,
    /// A module was removed from the slot named by [`SensorEvent::source`].
    ModuleRemoved,
    /// BQ27220 state of charge, `0..=100`.
    BatteryLevel(u8),
    /// External voice/LLM Activity input. Accepts `Listening`, `Thinking`,
    /// `Speaking`, `Error` and `Idle`; other expressions are ignored.
    Activity(BallExpression),
}

/// One timestamped sensor event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorEvent {
    pub at_ms: u64,
    pub source: EventSource,
    pub kind: EventKind,
}

/// Why the current expression was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactionReason {
    Boot,
    Idle,
    Activity,
    ShakeLarge,
    ShakeSmall,
    ShakeBursts,
    TouchPet,
    TouchPoke,
    Light,
    Presence,
    KeyLeft,
    KeyRight,
    BothKeys,
    ButtonPower,
    Magnetic,
    ModuleInsert,
    ModuleRemove,
    /// The resting face changed because of the battery.
    Battery,
    Error,
}

/// The resolved expression plus its provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffectState {
    pub expression: BallExpression,
    pub intensity: u8,
    pub since_ms: u64,
    pub reason: ReactionReason,
}

/// A motor pattern from RFC 0004 section 6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapticPattern {
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

/// A one-shot system action for the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    Hub,
    StopActivity,
    StartListening,
}

/// The result of [`AffectEngine::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffectTick {
    pub state: AffectState,
    pub changed: bool,
    pub haptic: Option<HapticPattern>,
    pub command: Option<SystemCommand>,
}

// ---------------------------------------------------------------------------
// Thresholds and durations
// ---------------------------------------------------------------------------

const ACCEL_REST_MG: i32 = 80;
const ACCEL_SMALL_MG: i32 = 300;
const ACCEL_LARGE_MG: i32 = 1_200;
const GYRO_REST_DPS: i32 = 20;
const GYRO_LARGE_DPS: i32 = 200;
const LARGE_PEAK_WINDOW_MS: u64 = 200;
const REST_CONFIRM_MS: u64 = 300;
const BURST_GAP_MIN_MS: u64 = 300;
const BURST_GAP_MAX_MS: u64 = 800;
const BURST_WINDOW_MS: u64 = 15_000;
const ANGRY_BURST_COUNT: u8 = 5;
const PANIC_HOLD_MS: u64 = 1_500;
const PANIC_STILL_MS: u64 = 5_000;
const PANIC_CONFUSED_MS: u64 = 3_000;
const SMALL_CONFUSED_MS: u64 = 2_500;
const ANGRY_HOLD_MS: u64 = 10_000;
const ANGRY_COOLDOWN_MS: u64 = 30_000;

const TOUCH_PET_HOLD_MS: u64 = 1_500;
const TOUCH_POKE_MS: u64 = 1_500;
const RAPID_POKE_WINDOW_MS: u64 = 1_500;
const RAPID_POKE_COUNT: u8 = 3;

const KEY_HAPPY_MS: u64 = 1_500;
const KEY_PUZZLED_MS: u64 = 1_500;
const KEY_TRIPLE_WINDOW_MS: u64 = 1_500;
const KEY_TRIPLE_COUNT: u8 = 3;
const HUG_WINDOW_MS: u64 = 300;
const HUG_MS: u64 = 2_000;

const LIGHT_HISTORY: usize = 16;
const LIGHT_FAST_WINDOW_MS: u64 = 300;
const LIGHT_FAST_DELTA: i32 = 25;
const LIGHT_BRIGHT_MS: u64 = 1_500;
const LIGHT_DARK_MS: u64 = 2_000;
const LIGHT_DARK_LEVEL: u8 = 10;
const LIGHT_WAKE_LEVEL: u8 = 20;
const NIGHT_SLEEP_MS: u64 = 60_000;
const DAWN_WINDOW_MS: u64 = 30_000;
const DAWN_RISE: u8 = 20;
const DAWN_CURIOUS_MS: u64 = 2_000;

const PIR_DEBOUNCE_MS: u64 = 200;
const PIR_SUSTAINED_MS: u64 = 10_000;
const PIR_SATISFIED_MS: u64 = 1_500;
const PIR_SATISFIED_COOLDOWN_MS: u64 = 30_000;
const PIR_ABSENCE_SLEEP_MS: u64 = 60_000;
const PIR_CURIOUS_MS: u64 = 3_000;

const BMM_APPROACH_MS: u64 = 2_000;
const BMM_CONTACT_MS: u64 = 1_500;
const BMM_RELEASE_MS: u64 = 2_000;
const BMM_ORBIT_MS: u64 = 1_200;
const BMM_EDGE_MS: u64 = 1_500;
const BMM_CONFIDENCE_MIN: u8 = 50;

const MODULE_RIGHT_INSERT_SURPRISED_MS: u64 = 500;
const MODULE_RIGHT_INSERT_CURIOUS_MS: u64 = 1_500;
const MODULE_RIGHT_INSERT_HAPPY_MS: u64 = 1_200;
const MODULE_RIGHT_REMOVE_SURPRISED_MS: u64 = 400;
const MODULE_RIGHT_REMOVE_CONFUSED_MS: u64 = 2_000;
const MODULE_LEFT_INSERT_CURIOUS_MS: u64 = 1_500;
const MODULE_LEFT_REMOVE_CONFUSED_MS: u64 = 1_500;

const HAPTIC_RATE_LIMIT_MS: u64 = 150;

/// Tire below this state of charge.
const BATTERY_TIRED_PCT: u8 = 15;
/// Leave `Tired` at or above this level (hysteresis).
const BATTERY_TIRED_CLEAR_PCT: u8 = 18;
/// Half-shut eyes below this state of charge.
const BATTERY_DYING_PCT: u8 = 5;
/// Leave `Dying` at or above this level (hysteresis).
const BATTERY_DYING_CLEAR_PCT: u8 = 8;

const PRIO_AMBIENT: u8 = 1;
const PRIO_PHYSICAL: u8 = 2;
const PRIO_ANGRY: u8 = 3;
const PRIO_PANIC: u8 = 4;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Reaction {
    expression: BallExpression,
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
    expression: BallExpression,
    reason: ReactionReason,
    duration_ms: u64,
}

impl ModuleStep {
    const EMPTY: Self = Self {
        expression: BallExpression::Idle,
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
    /// Last accel peak above the large threshold.
    last_large_peak_ms: Option<u64>,
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
    last_expression: BallExpression,
    fallback_since_ms: u64,
    idle_expression: BallExpression,
    activity: Option<BallExpression>,
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
    magnetic_contacted: bool,
    module_right_present: bool,
    module_left_present: bool,
    /// State-of-charge driven resting face; `None` is the normal idle.
    battery_expression: Option<BallExpression>,
}

impl AffectEngine {
    /// Create an engine whose first idle timestamp is `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms,
            last_expression: BallExpression::Idle,
            fallback_since_ms: now_ms,
            idle_expression: BallExpression::Idle,
            activity: None,
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
            magnetic_contacted: false,
            module_right_present: false,
            module_left_present: false,
            battery_expression: None,
        }
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
            EventKind::ModuleInserted => self.on_module_inserted(at, event.source),
            EventKind::ModuleRemoved => self.on_module_removed(at, event.source),
            EventKind::BatteryLevel(percent) => self.on_battery_level(at, percent),
            EventKind::Activity(expression) => self.on_activity(at, expression),
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
    /// Only the reaction's end time is evaluated; the ambient timers (shake
    /// bursts, PIR presence, night) still advance on the next real `tick`. This
    /// is what the runtime needs to merge layers immediately after an event.
    pub fn snapshot_at(&self, now_ms: u64) -> AffectState {
        let mut state = self.snapshot();
        if let Some(reaction) = self.reaction {
            if now_ms >= reaction.ends_ms {
                // The reaction has expired but `advance` has not run yet.
                state = self.snapshot_without_reaction();
            }
        }
        state
    }

    fn snapshot_without_reaction(&self) -> AffectState {
        if self.error_active {
            return AffectState {
                expression: BallExpression::Error,
                intensity: expression_intensity(BallExpression::Error),
                since_ms: self.error_since_ms,
                reason: ReactionReason::Error,
            };
        }
        if let Some(sequence) = self.module_sequence {
            let step = sequence.steps[sequence.index as usize];
            return AffectState {
                expression: step.expression,
                intensity: expression_intensity(step.expression),
                since_ms: sequence.stage_started_ms,
                reason: step.reason,
            };
        }
        if let Some(expression) = self.activity {
            return AffectState {
                expression,
                intensity: expression_intensity(expression),
                since_ms: self.fallback_since_ms,
                reason: ReactionReason::Activity,
            };
        }
        if let Some(expression) = self.battery_expression {
            return AffectState {
                expression,
                intensity: expression_intensity(expression),
                since_ms: self.fallback_since_ms,
                reason: ReactionReason::Battery,
            };
        }
        AffectState {
            expression: self.idle_expression,
            intensity: expression_intensity(self.idle_expression),
            since_ms: self.fallback_since_ms,
            reason: ReactionReason::Idle,
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

    fn resolve(&self) -> (BallExpression, ReactionReason, u64) {
        if self.error_active {
            return (
                BallExpression::Error,
                ReactionReason::Error,
                self.error_since_ms,
            );
        }
        if let Some(reaction) = self.reaction {
            return (reaction.expression, reaction.reason, reaction.started_ms);
        }
        if let Some(sequence) = self.module_sequence {
            let step = sequence.steps[sequence.index as usize];
            return (step.expression, step.reason, sequence.stage_started_ms);
        }
        if let Some(expression) = self.activity {
            if self.panic_followup.is_some() {
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
                BallExpression::Confused,
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
                if reaction.expression == BallExpression::Confused
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

    fn advance_module_sequence(&mut self, now: u64) {
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

    fn advance_pir(&mut self, now: u64) {
        if self.pir_raw != self.pir_debounced {
            let confirm_at = self.pir_raw_since.saturating_add(PIR_DEBOUNCE_MS);
            if now >= confirm_at {
                let rising = self.pir_raw;
                self.pir_debounced = rising;
                if rising {
                    self.pir_presence_since = Some(confirm_at);
                    self.pir_sustained_fired = false;
                    self.pir_absence_since = None;
                    self.wake(confirm_at);
                    self.fallback_since_ms = confirm_at;
                    let _ = self.try_start_reaction(
                        BallExpression::Curious,
                        ReactionReason::Presence,
                        PRIO_AMBIENT,
                        confirm_at,
                        PIR_CURIOUS_MS,
                        None,
                    );
                } else {
                    self.pir_presence_since = None;
                    self.pir_absence_since = Some(confirm_at);
                    self.pir_sustained_fired = false;
                }
            }
        }

        if self.pir_debounced {
            if let Some(since) = self.pir_presence_since {
                let fire_at = since.saturating_add(PIR_SUSTAINED_MS);
                if now >= fire_at && !self.pir_sustained_fired {
                    let allowed = match self.pir_last_satisfied_ms {
                        Some(last) => fire_at >= last.saturating_add(PIR_SATISFIED_COOLDOWN_MS),
                        None => true,
                    };
                    if allowed
                        && self.try_start_reaction(
                            BallExpression::Satisfied,
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
            let sleep_at = since.saturating_add(PIR_ABSENCE_SLEEP_MS);
            if !self.pir_debounced
                && now >= sleep_at
                && self.idle_expression != BallExpression::Sleep
            {
                self.idle_expression = BallExpression::Sleep;
                self.fallback_since_ms = sleep_at;
            }
        }
    }

    fn advance_dawn(&mut self, now: u64) {
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
                BallExpression::Curious,
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

    fn advance_night(&mut self, now: u64) {
        if let Some(since) = self.dark_since {
            let sleep_at = since.saturating_add(NIGHT_SLEEP_MS);
            if now >= sleep_at && self.idle_expression != BallExpression::Sleep {
                self.idle_expression = BallExpression::Sleep;
                self.fallback_since_ms = sleep_at;
            }
        }
    }

    fn extend_key_hold(&mut self, now: u64) {
        if !self.key_left_down {
            return;
        }
        if let Some(reaction) = &mut self.reaction {
            if reaction.reason == ReactionReason::KeyLeft
                && reaction.expression == BallExpression::Happy
                && reaction.priority == PRIO_PHYSICAL
            {
                reaction.ends_ms = now.saturating_add(KEY_HAPPY_MS);
            }
        }
    }

    fn wake(&mut self, at: u64) {
        self.dark_since = None;
        if self.idle_expression == BallExpression::Sleep {
            self.idle_expression = BallExpression::Idle;
            self.fallback_since_ms = at;
        }
    }

    // -----------------------------------------------------------------------
    // Reaction selection
    // -----------------------------------------------------------------------

    fn try_start_reaction(
        &mut self,
        expression: BallExpression,
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
            expression: BallExpression::Panic,
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
        self.shake.last_large_peak_ms = None;
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
            expression: BallExpression::Angry,
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
    // IMU
    // -----------------------------------------------------------------------

    fn on_accel(&mut self, at: u64, ax_mg: i16, ay_mg: i16, az_mg: i16) {
        let dynamic_mg = accel_dynamic_mg(ax_mg, ay_mg, az_mg);

        if dynamic_mg < ACCEL_REST_MG {
            self.shake.burst_above_small = false;
            return;
        }

        self.note_motion(at);

        if dynamic_mg > ACCEL_LARGE_MG {
            self.shake.burst_above_small = false;
            self.shake.burst_large = true;
            self.on_large_peak(at);
        } else if dynamic_mg >= ACCEL_SMALL_MG {
            if !self.shake.burst_above_small {
                self.shake.burst_above_small = true;
                self.shake.burst_small_peaks = self.shake.burst_small_peaks.saturating_add(1);
            }
            self.on_small_motion(at);
        } else {
            self.shake.burst_above_small = false;
        }
    }

    fn on_gyro(&mut self, at: u64, gx_dps: i16, gy_dps: i16, gz_dps: i16) {
        let max_dps = gyro_max_dps(gx_dps, gy_dps, gz_dps);
        if max_dps < GYRO_REST_DPS {
            return;
        }
        self.note_motion(at);
        if max_dps > GYRO_LARGE_DPS {
            self.shake.burst_large = true;
            self.shake.burst_confused_emitted = true;
            if !self.is_panic_active(at) {
                self.trigger_panic(at);
            }
        }
    }

    fn note_motion(&mut self, at: u64) {
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
                && reaction.expression == BallExpression::Confused
                && at < reaction.ends_ms
            {
                reaction.ends_ms = at.saturating_add(SMALL_CONFUSED_MS);
            }
        }
    }

    fn on_large_peak(&mut self, at: u64) {
        self.shake.burst_confused_emitted = true;
        if self.is_panic_active(at) {
            return;
        }
        if let Some(previous) = self.shake.last_large_peak_ms {
            if at.saturating_sub(previous) <= LARGE_PEAK_WINDOW_MS {
                self.shake.last_large_peak_ms = None;
                self.trigger_panic(at);
                return;
            }
        }
        self.shake.last_large_peak_ms = Some(at);
    }

    fn on_small_motion(&mut self, at: u64) {
        if self.shake.burst_large {
            return;
        }

        if let Some(reaction) = self.reaction {
            if reaction.reason == ReactionReason::ShakeSmall
                && reaction.expression == BallExpression::Confused
                && at < reaction.ends_ms
            {
                if let Some(active) = &mut self.reaction {
                    active.ends_ms = at.saturating_add(SMALL_CONFUSED_MS);
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
            BallExpression::Confused,
            ReactionReason::ShakeSmall,
            PRIO_PHYSICAL,
            at,
            SMALL_CONFUSED_MS,
            haptic,
        ) {
            self.shake.burst_confused_emitted = true;
        }
    }

    fn update_burst(&mut self, now: u64) {
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

    // -----------------------------------------------------------------------
    // Touch
    // -----------------------------------------------------------------------

    fn on_touch_pet(&mut self, at: u64) {
        self.wake(at);
        if let Some(reaction) = self.reaction {
            if reaction.reason == ReactionReason::TouchPet
                && reaction.expression == BallExpression::Happy
                && at < reaction.ends_ms
            {
                if let Some(active) = &mut self.reaction {
                    active.ends_ms = at.saturating_add(TOUCH_PET_HOLD_MS);
                }
                return;
            }
        }
        let _ = self.try_start_reaction(
            BallExpression::Happy,
            ReactionReason::TouchPet,
            PRIO_PHYSICAL,
            at,
            TOUCH_PET_HOLD_MS,
            Some(HapticPattern::Happy),
        );
    }

    fn on_touch_poke(&mut self, at: u64) {
        self.wake(at);
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
            BallExpression::Puzzled,
            ReactionReason::TouchPoke,
            PRIO_PHYSICAL,
            at,
            TOUCH_POKE_MS,
            Some(HapticPattern::Puzzled),
        );
    }

    // -----------------------------------------------------------------------
    // Light and PIR
    // -----------------------------------------------------------------------

    fn on_light_level(&mut self, at: u64, level: u8) {
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
                let delta = level as i32 - oldest.level as i32;
                if delta >= LIGHT_FAST_DELTA {
                    let _ = self.try_start_reaction(
                        BallExpression::Surprised,
                        ReactionReason::Light,
                        PRIO_AMBIENT,
                        at,
                        LIGHT_BRIGHT_MS,
                        Some(HapticPattern::Surprised),
                    );
                    self.reanchor_dawn(at, level);
                } else if delta <= -LIGHT_FAST_DELTA {
                    let _ = self.try_start_reaction(
                        BallExpression::Confused,
                        ReactionReason::Light,
                        PRIO_AMBIENT,
                        at,
                        LIGHT_DARK_MS,
                        Some(HapticPattern::Confused),
                    );
                    self.reanchor_dawn(at, level);
                }
            }
        }

        if self.dawn_ref_at.is_none() {
            self.reanchor_dawn(at, level);
        }
        self.light_level = Some(level);
    }

    fn reanchor_dawn(&mut self, at: u64, level: u8) {
        self.dawn_ref_level = level;
        self.dawn_ref_at = Some(at);
    }

    fn on_presence(&mut self, at: u64, present: bool) {
        if present == self.pir_raw {
            return;
        }
        self.pir_raw = present;
        self.pir_raw_since = at;
    }

    // -----------------------------------------------------------------------
    // Keys and buttons
    // -----------------------------------------------------------------------

    fn on_key_down(&mut self, at: u64, source: EventSource) {
        match source {
            EventSource::KeyLeft => self.on_key_left_down(at),
            EventSource::KeyRight => self.on_key_right_down(at),
            _ => {}
        }
    }

    fn on_key_up(&mut self, at: u64, source: EventSource) {
        match source {
            EventSource::KeyLeft => {
                if !self.key_left_down {
                    return;
                }
                self.key_left_down = false;
                self.key_left_down_at = None;
                if let Some(reaction) = &mut self.reaction {
                    if reaction.reason == ReactionReason::KeyLeft
                        && reaction.expression == BallExpression::Happy
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

    fn on_key_left_down(&mut self, at: u64) {
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
            BallExpression::Happy,
            ReactionReason::KeyLeft,
            PRIO_PHYSICAL,
            at,
            KEY_HAPPY_MS,
            Some(HapticPattern::Happy),
        );
    }

    fn on_key_right_down(&mut self, at: u64) {
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
            BallExpression::Puzzled,
            ReactionReason::KeyRight,
            PRIO_PHYSICAL,
            at,
            KEY_PUZZLED_MS,
            Some(HapticPattern::Puzzled),
        );
    }

    fn start_hug(&mut self, at: u64) {
        self.r_key_count = 0;
        self.poke_count = 0;
        let _ = self.try_start_reaction(
            BallExpression::Satisfied,
            ReactionReason::BothKeys,
            PRIO_PHYSICAL,
            at,
            HUG_MS,
            Some(HapticPattern::Satisfied),
        );
    }

    fn on_button_short(&mut self, at: u64, source: EventSource) {
        if source != EventSource::ButtonPower {
            return;
        }
        if !self.error_active {
            self.activity = Some(BallExpression::Listening);
            self.fallback_since_ms = at;
        }
        self.pending_command = Some(SystemCommand::StartListening);
        self.queue_haptic(HapticPattern::Listening);
    }

    fn on_button_long(&mut self, _at: u64, source: EventSource) {
        if source != EventSource::ButtonPower {
            return;
        }
        // No expression change and no pattern exists for this generic tick in
        // the frozen HapticPattern set.
        self.pending_command = Some(SystemCommand::Hub);
    }

    fn on_button_double(&mut self, at: u64, source: EventSource) {
        if source != EventSource::ButtonPower {
            return;
        }
        self.activity = None;
        self.fallback_since_ms = at;
        self.pending_command = Some(SystemCommand::StopActivity);
    }

    // -----------------------------------------------------------------------
    // External Activity input
    // -----------------------------------------------------------------------

    fn on_activity(&mut self, at: u64, expression: BallExpression) {
        match expression {
            BallExpression::Listening | BallExpression::Thinking | BallExpression::Speaking => {
                // Error blocks all Activity updates except Activity(Idle).
                if !self.error_active {
                    self.activity = Some(expression);
                    self.fallback_since_ms = at;
                }
            }
            BallExpression::Error => self.enter_error(at),
            BallExpression::Idle => {
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
    /// The battery is a *baseline*, not a reaction: it changes what the ball
    /// looks like while resting. Hysteresis keeps a gauge flickering around a
    /// threshold from flapping the face.
    fn on_battery_level(&mut self, at: u64, percent: u8) {
        let percent = percent.min(100);
        let next = match self.battery_expression {
            Some(BallExpression::Dying) => {
                if percent >= BATTERY_DYING_CLEAR_PCT {
                    if percent <= BATTERY_TIRED_PCT {
                        Some(BallExpression::Tired)
                    } else {
                        None
                    }
                } else {
                    Some(BallExpression::Dying)
                }
            }
            Some(BallExpression::Tired) => {
                if percent >= BATTERY_TIRED_CLEAR_PCT {
                    None
                } else if percent <= BATTERY_DYING_PCT {
                    Some(BallExpression::Dying)
                } else {
                    Some(BallExpression::Tired)
                }
            }
            _ => {
                if percent <= BATTERY_DYING_PCT {
                    Some(BallExpression::Dying)
                } else if percent <= BATTERY_TIRED_PCT {
                    Some(BallExpression::Tired)
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
    // BMM150
    // -----------------------------------------------------------------------

    fn on_magnetic_approach(&mut self, at: u64) {
        let _ = self.try_start_reaction(
            BallExpression::Curious,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_APPROACH_MS,
            None,
        );
    }

    fn on_magnetic_contact(&mut self, at: u64) {
        if self.magnetic_contacted {
            return;
        }
        self.magnetic_contacted = true;
        let _ = self.try_start_reaction(
            BallExpression::Satisfied,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_CONTACT_MS,
            Some(HapticPattern::MagneticContact),
        );
    }

    fn on_magnetic_release(&mut self, at: u64) {
        if !self.magnetic_contacted {
            return;
        }
        self.magnetic_contacted = false;
        let _ = self.try_start_reaction(
            BallExpression::Confused,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_RELEASE_MS,
            Some(HapticPattern::Confused),
        );
    }

    fn on_magnetic_orbit(&mut self, at: u64) {
        let _ = self.try_start_reaction(
            BallExpression::Surprised,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_ORBIT_MS,
            Some(HapticPattern::Surprised),
        );
    }

    fn on_magnetic_edge(&mut self, at: u64, confidence: u8) {
        if confidence < BMM_CONFIDENCE_MIN {
            return;
        }
        let _ = self.try_start_reaction(
            BallExpression::Puzzled,
            ReactionReason::Magnetic,
            PRIO_PHYSICAL,
            at,
            BMM_EDGE_MS,
            Some(HapticPattern::Puzzled),
        );
    }

    // -----------------------------------------------------------------------
    // Module slot
    // -----------------------------------------------------------------------

    fn on_module_inserted(&mut self, at: u64, source: EventSource) {
        match source {
            EventSource::ModuleRight => {
                if self.module_right_present {
                    return;
                }
                self.module_right_present = true;
                let steps = [
                    ModuleStep {
                        expression: BallExpression::Surprised,
                        reason: ReactionReason::ModuleInsert,
                        duration_ms: MODULE_RIGHT_INSERT_SURPRISED_MS,
                    },
                    ModuleStep {
                        expression: BallExpression::Curious,
                        reason: ReactionReason::ModuleInsert,
                        duration_ms: MODULE_RIGHT_INSERT_CURIOUS_MS,
                    },
                    ModuleStep {
                        expression: BallExpression::Happy,
                        reason: ReactionReason::ModuleInsert,
                        duration_ms: MODULE_RIGHT_INSERT_HAPPY_MS,
                    },
                ];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleInsert);
            }
            EventSource::ModuleLeft => {
                if self.module_left_present {
                    return;
                }
                self.module_left_present = true;
                let steps = [ModuleStep {
                    expression: BallExpression::Curious,
                    reason: ReactionReason::ModuleInsert,
                    duration_ms: MODULE_LEFT_INSERT_CURIOUS_MS,
                }];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleInsert);
            }
            _ => {}
        }
    }

    fn on_module_removed(&mut self, at: u64, source: EventSource) {
        match source {
            EventSource::ModuleRight => {
                if !self.module_right_present {
                    return;
                }
                self.module_right_present = false;
                let steps = [
                    ModuleStep {
                        expression: BallExpression::Surprised,
                        reason: ReactionReason::ModuleRemove,
                        duration_ms: MODULE_RIGHT_REMOVE_SURPRISED_MS,
                    },
                    ModuleStep {
                        expression: BallExpression::Confused,
                        reason: ReactionReason::ModuleRemove,
                        duration_ms: MODULE_RIGHT_REMOVE_CONFUSED_MS,
                    },
                ];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleRemove);
            }
            EventSource::ModuleLeft => {
                if !self.module_left_present {
                    return;
                }
                self.module_left_present = false;
                let steps = [ModuleStep {
                    expression: BallExpression::Confused,
                    reason: ReactionReason::ModuleRemove,
                    duration_ms: MODULE_LEFT_REMOVE_CONFUSED_MS,
                }];
                self.start_module_sequence(at, &steps, HapticPattern::ModuleRemove);
            }
            _ => {}
        }
    }

    fn start_module_sequence(&mut self, at: u64, steps: &[ModuleStep], haptic: HapticPattern) {
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

    // -----------------------------------------------------------------------
    // Haptics
    // -----------------------------------------------------------------------

    fn queue_haptic(&mut self, pattern: HapticPattern) {
        // Error is the top layer; no lower-layer haptic may run while it is
        // active. Error itself replaces any pending lower pattern.
        if self.error_active && pattern != HapticPattern::Error {
            return;
        }
        let priority = haptic_priority(pattern);
        match self.pending_haptic {
            None => self.pending_haptic = Some(pattern),
            Some(existing) => {
                if priority >= haptic_priority(existing) {
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

fn haptic_priority(pattern: HapticPattern) -> u8 {
    match pattern {
        HapticPattern::Error => 3,
        HapticPattern::Panic => 2,
        _ => 1,
    }
}

/// Intensity mirrors the plan's haptic intensity column where one exists.
/// Idle and Sleep are zero; the remaining values are stable presentation
/// defaults for the renderer.
fn expression_intensity(expression: BallExpression) -> u8 {
    match expression {
        BallExpression::Idle | BallExpression::Sleep => 0,
        BallExpression::Curious => 40,
        BallExpression::Happy => 50,
        BallExpression::Puzzled => 60,
        BallExpression::Confused => 45,
        BallExpression::Surprised => 70,
        BallExpression::Panic => 100,
        BallExpression::Angry => 90,
        BallExpression::Satisfied => 55,
        BallExpression::Listening => 60,
        BallExpression::Thinking => 35,
        BallExpression::Speaking => 55,
        BallExpression::Error => 100,
        BallExpression::Tired => 25,
        BallExpression::Dying => 15,
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
