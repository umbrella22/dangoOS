//! Human-feeling tap personality.
//!
//! The fox does not support grab/drag: product decision, the fixed plane
//! keeps it at the panel centre. A finger that stays or strokes is petting:
//! the fox closes her eyes contentedly, squashes under the hand and leans
//! with the stroke, and the press never counts as a tap. Short presses are
//! the tap ladder:
//!
//! - a tap is noticed after a short beat, not in the same frame;
//! - one tap is curiosity, two are puzzlement, three or four are mild
//!   annoyance, and a frantic burst makes it angry;
//! - a tap during a burst waits for the gesture to settle before the face
//!   answers;
//! - a reaction has a hold time, and anger has a cooldown.
//!
//! The engine is pure `core` Rust with fixed-size storage and integer time; the
//! same code runs in host tests and on the device.

use crate::companion::behavior::affect::HapticPattern;
use crate::companion::behavior::Expression;

/// Screen center used by the 480x480 panel.
pub const SCREEN_CENTER_X: i32 = 240;
pub const SCREEN_CENTER_Y: i32 = 240;

/// Longest press that still counts as a tap.
pub const TAP_MAX_MS: u64 = 320;
/// Largest pointer travel during a tap.
pub const TAP_MAX_MOVE_PX: i32 = 14;
/// Window for the tap ladder (one/two/three/four taps).
pub const TAP_WINDOW_MS: u64 = 2_500;
/// Window for the angry burst.
pub const FRANTIC_WINDOW_MS: u64 = 1_200;
/// Taps inside [`FRANTIC_WINDOW_MS`] that make the fox angry.
pub const ANGRY_TAP_COUNT: u8 = 5;
/// Delay before the first reaction: a creature turns its head, it does not
/// switch modes.
pub const FIRST_REACTION_DELAY_MS: u64 = 220;
/// Delay after the last tap of a burst, so the face answers the gesture.
pub const BURST_SETTLE_DELAY_MS: u64 = 380;
/// After an angry reaction, taps only annoy the fox again.
pub const ANGRY_COOLDOWN_MS: u64 = 6_000;
/// Visual tap pulse duration.
pub const TAP_PULSE_MS: u64 = 280;
/// A press held this long is petting, even without moving.
pub const PET_HOLD_MS: u64 = 450;
/// Or a press that travels this far: a stroke.
pub const PET_STROKE_PX: i32 = 40;
/// Petting stays on screen this long after the finger lifts.
pub const PET_AFTER_MS: u64 = 1_800;
/// Squash of a hand resting on the head, `0..=1`.
const PET_REST_SQUASH: f32 = 0.35;
/// Squash added per pixel of stroke travel.
const PET_SQUASH_PER_PX: f32 = 0.02;
/// Stroke squash half-life, ms.
const PET_SQUASH_DECAY_MS: f32 = 180.0;
/// Lean toward the stroke: pixels of lean per pixel of finger offset.
const PET_LEAN_PER_PX: f32 = 0.06;
/// Largest lean, canvas pixels.
pub const PET_LEAN_MAX_PX: f32 = 6.0;

/// Hold time for one personality reaction.
const fn reaction_hold_ms(reaction: PersonaReaction) -> u64 {
    match reaction {
        PersonaReaction::Curious => 1_500,
        PersonaReaction::Puzzled => 1_800,
        PersonaReaction::Annoyed => 2_000,
        PersonaReaction::Angry => 4_000,
        PersonaReaction::Petted => PET_AFTER_MS,
    }
}

/// Why the personality layer changed the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonaReaction {
    /// A single tap: "what was that?".
    Curious,
    /// Two taps: "again?".
    Puzzled,
    /// Three or four taps: "please stop".
    Annoyed,
    /// A finger resting or stroking: contented, eyes shut.
    Petted,
    /// A frantic burst: genuinely angry.
    Angry,
}

impl PersonaReaction {
    /// Expression shown while the reaction is active.
    pub const fn expression(self) -> Expression {
        match self {
            Self::Curious => Expression::Curious,
            Self::Puzzled => Expression::Puzzled,
            Self::Annoyed => Expression::Confused,
            Self::Angry => Expression::Angry,
            Self::Petted => Expression::Satisfied,
        }
    }

    /// Haptic pattern paired with the reaction, if any.
    ///
    /// A single tap gets no buzz on purpose: a real thing nudged by a finger
    /// does not vibrate. Anger does.
    const fn haptic(self) -> Option<HapticPattern> {
        match self {
            Self::Curious => None,
            Self::Puzzled => Some(HapticPattern::Puzzled),
            Self::Annoyed => Some(HapticPattern::Confused),
            Self::Angry => Some(HapticPattern::Angry),
            Self::Petted => Some(HapticPattern::Satisfied),
        }
    }
}

/// One pointer sample from the input task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerSample {
    pub at_ms: u64,
    pub x: i32,
    pub y: i32,
    pub pressed: bool,
}

/// The personality layer's contribution to the rendered frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PersonaOutput {
    /// Expression to show instead of the ambient/affect expression.
    pub expression: Option<Expression>,
    /// Active reaction, for logging and tests.
    pub reaction: Option<PersonaReaction>,
    /// One-shot haptic request.
    pub haptic: Option<HapticPattern>,
    /// Decaying tap pulse, `0..=1`.
    pub pulse: f32,
    /// Gaze offset in source units toward the finger, `-8..=8`.
    pub look_x: f32,
    /// Squash under a petting hand, `0..=1`.
    pub pet: f32,
    /// Lean with the stroke, canvas pixels.
    pub lean_x: f32,
}

impl PersonaOutput {
    /// The neutral output: no reaction, no pulse, eyes forward.
    pub const IDLE: Self = Self {
        expression: None,
        reaction: None,
        haptic: None,
        pulse: 0.0,
        look_x: 0.0,
        pet: 0.0,
        lean_x: 0.0,
    };
}

/// One queued reaction waiting for its human-sized delay.
#[derive(Clone, Copy, Debug)]
struct PendingReaction {
    reaction: PersonaReaction,
    at_ms: u64,
}

/// One reaction currently on screen.
#[derive(Clone, Copy, Debug)]
struct ActiveReaction {
    reaction: PersonaReaction,
    ends_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Pressed,
}

/// Deterministic touch personality.
pub struct PersonalityEngine {
    now_ms: u64,
    phase: Phase,

    press_start_ms: u64,
    press_x: i32,
    press_y: i32,
    last_x: i32,
    last_y: i32,
    press_travel: i32,
    look_x: f32,

    /// The current press is petting.
    petting: bool,
    /// Decaying squash from stroke travel.
    stroke: f32,
    /// Eased pet squash and lean actually shown.
    pet: f32,
    lean_x: f32,
    eased_ms: u64,

    tap_times: [u64; 8],
    tap_count: u8,

    pending: Option<PendingReaction>,
    active: Option<ActiveReaction>,
    angry_cooldown_until: u64,

    pulse_start_ms: Option<u64>,
    ignore_until_ms: u64,
    /// One-shot haptic queued by the reaction that just started.
    haptic_pending: Option<HapticPattern>,
}

impl PersonalityEngine {
    /// Create an engine whose clock starts at `now_ms`.
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms,
            phase: Phase::Idle,
            press_start_ms: 0,
            press_x: 0,
            press_y: 0,
            last_x: 0,
            last_y: 0,
            press_travel: 0,
            look_x: 0.0,
            petting: false,
            stroke: 0.0,
            pet: 0.0,
            lean_x: 0.0,
            eased_ms: now_ms,
            tap_times: [0; 8],
            tap_count: 0,
            pending: None,
            active: None,
            angry_cooldown_until: 0,
            pulse_start_ms: None,
            ignore_until_ms: 0,
            haptic_pending: None,
        }
    }

    /// Discard every pointer sample before `until_ms`.
    pub fn ignore_until(&mut self, until_ms: u64) {
        self.ignore_until_ms = self.ignore_until_ms.max(until_ms);
        if self.phase != Phase::Idle {
            self.phase = Phase::Idle;
            self.pending = None;
            self.petting = false;
        }
    }

    /// Whether a sample at `at_ms` would be processed.
    pub fn accepts(&self, at_ms: u64) -> bool {
        at_ms >= self.ignore_until_ms
    }

    /// The fox is fixed at the screen center.
    pub fn fox_center(&self) -> (i32, i32) {
        (SCREEN_CENTER_X, SCREEN_CENTER_Y)
    }

    /// Apply one pointer sample.
    pub fn on_pointer(&mut self, sample: PointerSample) -> PersonaOutput {
        let at = sample.at_ms.max(self.now_ms);
        self.now_ms = at;
        if at < self.ignore_until_ms {
            return self.output();
        }

        self.expire(at);
        if sample.pressed {
            self.on_pressed(at, sample.x, sample.y);
        } else {
            self.on_released(at, sample.x, sample.y);
        }
        self.output()
    }

    /// Advance timers and return the current output.
    pub fn tick(&mut self, now_ms: u64) -> PersonaOutput {
        let now = now_ms.max(self.now_ms);
        self.now_ms = now;
        self.expire(now);

        // Gaze eases toward the finger while touched and back to center
        // afterwards.
        // Only the horizontal component is used: the fox turns its head.
        let target_x = if self.phase == Phase::Idle {
            0.0
        } else {
            let (cx, cy) = self.fox_center();
            let dx = (self.last_x - cx) as f32;
            let len = integer_len(self.last_x - cx, self.last_y - cy).max(1) as f32;
            (dx / len) * 7.0
        };
        self.look_x += (target_x - self.look_x) * 0.25;

        if self.phase == Phase::Pressed
            && !self.petting
            && now.saturating_sub(self.press_start_ms) >= PET_HOLD_MS
        {
            self.start_petting();
        }
        self.ease_pet(now);

        self.output()
    }

    /// Ease the pet squash and lean toward their targets (time based).
    fn ease_pet(&mut self, now: u64) {
        let dt = now.saturating_sub(self.eased_ms).min(200) as f32;
        self.eased_ms = now;
        self.stroke *= half_life(dt, PET_SQUASH_DECAY_MS);
        let (squash, lean) = if self.petting && self.phase == Phase::Pressed {
            let lean = ((self.last_x - self.press_x) as f32 * PET_LEAN_PER_PX)
                .clamp(-PET_LEAN_MAX_PX, PET_LEAN_MAX_PX);
            ((PET_REST_SQUASH + self.stroke).min(1.0), lean)
        } else {
            (0.0, 0.0)
        };
        // About 60 ms to follow the hand, so a stroke reads as one motion.
        let k = 1.0 - half_life(dt, 60.0);
        self.pet += (squash - self.pet) * k;
        self.lean_x += (lean - self.lean_x) * k;
        if self.pet < 0.01 && squash == 0.0 {
            self.pet = 0.0;
        }
        if self.lean_x.abs() < 0.05 && lean == 0.0 {
            self.lean_x = 0.0;
        }
    }

    /// The press became petting: it soothes whatever the taps started.
    fn start_petting(&mut self) {
        self.petting = true;
        self.pending = None;
        self.tap_count = 0;
        self.active = Some(ActiveReaction {
            reaction: PersonaReaction::Petted,
            ends_ms: u64::MAX,
        });
        self.haptic_pending = PersonaReaction::Petted.haptic();
    }

    fn expire(&mut self, now: u64) {
        if let Some(pending) = self.pending {
            if now >= pending.at_ms {
                self.pending = None;
                self.active = Some(ActiveReaction {
                    reaction: pending.reaction,
                    ends_ms: now.saturating_add(reaction_hold_ms(pending.reaction)),
                });
                self.haptic_pending = pending.reaction.haptic();
                if pending.reaction == PersonaReaction::Angry {
                    self.angry_cooldown_until = now.saturating_add(ANGRY_COOLDOWN_MS);
                }
            }
        }
        if let Some(active) = self.active {
            if now >= active.ends_ms {
                self.active = None;
            }
        }
    }

    fn on_pressed(&mut self, at: u64, x: i32, y: i32) {
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Pressed;
                self.press_start_ms = at;
                self.press_x = x;
                self.press_y = y;
                self.last_x = x;
                self.last_y = y;
                self.press_travel = 0;
                self.petting = false;
                self.pulse_start_ms = Some(at);
            }
            Phase::Pressed => {
                self.track_press_move(x, y);
                if !self.petting
                    && (self.press_travel >= PET_STROKE_PX
                        || at.saturating_sub(self.press_start_ms) >= PET_HOLD_MS)
                {
                    self.start_petting();
                }
            }
        }
    }

    fn on_released(&mut self, at: u64, x: i32, y: i32) {
        if self.phase != Phase::Pressed {
            return;
        }
        self.track_press_move(x, y);
        self.last_x = x;
        self.last_y = y;
        let held = at.saturating_sub(self.press_start_ms);
        self.phase = Phase::Idle;
        if self.petting {
            self.petting = false;
            self.active = Some(ActiveReaction {
                reaction: PersonaReaction::Petted,
                ends_ms: at.saturating_add(PET_AFTER_MS),
            });
        } else if held <= TAP_MAX_MS && self.press_travel <= TAP_MAX_MOVE_PX {
            self.register_tap(at);
        }
    }

    fn track_press_move(&mut self, x: i32, y: i32) {
        let dx = x - self.last_x;
        let dy = y - self.last_y;
        if dx.abs() + dy.abs() > 2 {
            let moved = dx.abs() + dy.abs();
            self.press_travel = self.press_travel.saturating_add(moved);
            if self.petting {
                self.stroke = (self.stroke + moved as f32 * PET_SQUASH_PER_PX).min(1.0);
            }
            self.last_x = x;
            self.last_y = y;
        }
    }

    fn register_tap(&mut self, at: u64) {
        let mut kept = 0usize;
        let mut index = 0usize;
        while index < self.tap_count as usize {
            let time = self.tap_times[index];
            if at.saturating_sub(time) <= TAP_WINDOW_MS {
                self.tap_times[kept] = time;
                kept += 1;
            }
            index += 1;
        }
        self.tap_count = kept as u8;

        if self.tap_count as usize >= self.tap_times.len() {
            self.tap_times.copy_within(1.., 0);
            self.tap_count -= 1;
        }
        self.tap_times[self.tap_count as usize] = at;
        self.tap_count += 1;

        let frantic = self.taps_within(at, FRANTIC_WINDOW_MS);
        let count = self.tap_count;
        let reaction = if frantic >= ANGRY_TAP_COUNT {
            if at >= self.angry_cooldown_until {
                PersonaReaction::Angry
            } else {
                PersonaReaction::Annoyed
            }
        } else {
            match count {
                1 => PersonaReaction::Curious,
                2 => PersonaReaction::Puzzled,
                _ => PersonaReaction::Annoyed,
            }
        };

        if matches!(
            self.active.map(|active| active.reaction),
            Some(PersonaReaction::Angry)
        ) {
            return;
        }

        let delay = if count == 1 {
            FIRST_REACTION_DELAY_MS
        } else {
            BURST_SETTLE_DELAY_MS
        };
        self.pending = Some(PendingReaction {
            reaction,
            at_ms: at.saturating_add(delay),
        });
        self.pulse_start_ms = Some(at);
    }

    fn taps_within(&self, at: u64, window_ms: u64) -> u8 {
        let mut count = 0u8;
        let mut index = 0usize;
        while index < self.tap_count as usize {
            if at.saturating_sub(self.tap_times[index]) <= window_ms {
                count += 1;
            }
            index += 1;
        }
        count
    }

    fn output(&mut self) -> PersonaOutput {
        let pulse = match self.pulse_start_ms {
            Some(start) => {
                let elapsed = self.now_ms.saturating_sub(start);
                if elapsed >= TAP_PULSE_MS {
                    0.0
                } else {
                    let t = elapsed as f32 / TAP_PULSE_MS as f32;
                    if t < 0.15 {
                        t / 0.15
                    } else {
                        (1.0 - t) / 0.85
                    }
                }
            }
            None => 0.0,
        };

        let active = self.active.map(|active| active.reaction);
        PersonaOutput {
            expression: active.map(PersonaReaction::expression),
            reaction: active,
            haptic: self.haptic_pending.take(),
            pulse,
            look_x: self.look_x,
            pet: self.pet,
            lean_x: self.lean_x,
        }
    }
}

impl Default for PersonalityEngine {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Remaining fraction after `dt` ms with the given half-life (no `exp` in
/// `core`: a short polynomial is plenty for easing).
fn half_life(dt: f32, half_ms: f32) -> f32 {
    // 2^-x for x = dt / half, via 1 / (1 + x ln2 + (x ln2)^2 / 2), and halving
    // per whole half-life to keep the error small.
    let mut x = dt / half_ms;
    let mut scale = 1.0;
    while x > 1.0 {
        scale *= 0.5;
        x -= 1.0;
    }
    let y = x * core::f32::consts::LN_2;
    scale / (1.0 + y + y * y * 0.5)
}

/// Integer length of a 2D vector (no `sqrt` in `core`).
fn integer_len(x: i32, y: i32) -> i32 {
    let ax = x.abs();
    let ay = y.abs();
    let (long, short) = if ax >= ay { (ax, ay) } else { (ay, ax) };
    if long == 0 {
        return 0;
    }
    long + short / 2
}
