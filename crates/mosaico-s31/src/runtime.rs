//! Runtime arbiter: sensor events, personality and the rendered expression.
//!
//! This module is the glue H2 was missing. It owns three things that used to be
//! either absent or spread across tasks:
//!
//! 1. **The affect engine** — `ui_task` drains [`crate::sensor_task::SENSOR_EVENTS`]
//!    into it, so shake/pet/poke/light/PIR/keys/module/magnet/BMM150 and the
//!    battery gauge actually drive expressions.
//! 2. **The personality engine** — the pointer stream goes here, not to LVGL
//!    events, so a tap ladder, a grab and a drag are decided with timestamps
//!    from the input task and rendered by the same code on host and device.
//! 3. **The merged frame** — the arbiter pushes one expression and one pose into
//!    the [`BallControl`] per UI loop. Priority is:
//!
//!    ```text
//!    personality reaction  >  affect expression  >  resting expression
//!    ```
//!
//!    A held ball must keep struggling even while the affect engine still shows
//!    a poke from before the grab, and a scream from a hard shake must survive
//!    the tap that follows it.
//!
//! Haptics from both engines are collected here; the caller forwards them to
//! [`crate::haptic::play`], which owns the 150 ms rate limit and the hardware.
//!
//! # Host testability
//!
//! Every input is a plain value (`SensorEvent`, `PointerSample`, `now_ms`) and
//! the output goes through the shared [`BallControl`], so the merge rules are
//! unit-tested on the host without LVGL.

use alloc::rc::Rc;

use grok_ball::affect::{AffectEngine, AffectState, EventKind, HapticPattern, SensorEvent};
use grok_ball::personality::{PersonalityEngine, PersonaOutput, PersonaReaction, PointerSample};
use grok_ball::skin;
use grok_ball::types::BallExpression;

use crate::ball_widget::{BallControl, BallVisual};

/// The runtime state machine that turns sensors plus touch into one ball.
pub struct BallRuntime {
    affect: AffectEngine,
    personality: PersonalityEngine,
    control: Rc<BallControl>,
    /// Expression the affect engine currently resolves to.
    affect_expression: BallExpression,
    /// Latest personality output.
    persona: PersonaOutput,
    /// Haptics produced since the last drain, newest last.
    pending_haptics: [Option<HapticPattern>; 2],
    /// Entry count for `pending_haptics`.
    pending_len: usize,
    /// The merged expression the control currently shows.
    merged: BallExpression,
}

impl BallRuntime {
    /// Create the runtime at `now_ms`.
    pub fn new(now_ms: u64, control: Rc<BallControl>) -> Self {
        let mut runtime = Self {
            affect: AffectEngine::new(now_ms),
            personality: PersonalityEngine::new(now_ms),
            control,
            affect_expression: BallExpression::Idle,
            persona: PersonaOutput::IDLE,
            pending_haptics: [None; 2],
            pending_len: 0,
            merged: BallExpression::Idle,
        };
        runtime.publish();
        runtime
    }

    /// Discard every pointer sample before `until_ms`.
    ///
    /// The boot entrance calls this so a finger on the panel cannot interrupt
    /// the drop or queue a tap.
    pub fn ignore_touch_until(&mut self, until_ms: u64) {
        self.personality.ignore_until(until_ms);
    }

    /// The merged expression currently on screen.
    pub fn expression(&self) -> BallExpression {
        self.merged
    }

    /// The active personality reaction, for logs.
    pub fn reaction(&self) -> Option<PersonaReaction> {
        self.persona.reaction
    }

    /// The affect engine's unresolved state, for logs.
    pub fn affect_state(&self) -> AffectState {
        self.affect.snapshot()
    }

    /// Current pose as published to the widget.
    pub fn visual(&self) -> BallVisual {
        self.control.visual()
    }

    /// Apply one sensor event.
    pub fn on_sensor(&mut self, event: SensorEvent) {
        let at = event.at_ms;
        self.affect.on_event(event);
        self.refresh_affect(at);
        self.publish();
    }

    /// Apply one pointer sample from the input task.
    ///
    /// Returns `false` when the sample was discarded by the boot hold-off. The
    /// caller uses that to avoid tagging a latency sample for a touch that
    /// deliberately produced no visible response.
    pub fn on_pointer(&mut self, sample: PointerSample) -> bool {
        let at = sample.at_ms;
        if !self.personality.accepts(at) {
            return false;
        }
        let mut output = self.personality.on_pointer(sample);
        if let Some(pattern) = output.haptic.take() {
            self.push_haptic(pattern);
        }
        self.persona = output;
        self.refresh_affect(at);
        self.publish();
        true
    }

    /// Advance timers and merge the layers. Call once per UI loop.
    ///
    /// Returns the one-shot haptic requested by this tick, if any: a
    /// personality reaction is more immediate than an affect reaction, so it
    /// wins when both fire together.
    pub fn tick(&mut self, now_ms: u64) -> Option<HapticPattern> {
        let mut persona = self.personality.tick(now_ms);
        if let Some(pattern) = persona.haptic.take() {
            self.push_haptic(pattern);
        }
        self.persona = persona;

        let affect_tick = self.affect.tick(now_ms);
        self.affect_expression = affect_tick.state.expression;
        if let Some(pattern) = affect_tick.haptic {
            self.push_haptic(pattern);
        }

        self.publish();
        self.drain_haptics()
    }

    fn refresh_affect(&mut self, now_ms: u64) {
        self.affect_expression = self.affect.snapshot_at(now_ms).expression;
    }

    /// Push the merged expression and pose into the control.
    ///
    /// `BallControl::set_expression` and `set_visual` are change-gated, so this
    /// is free when nothing moved. The pose is quantized first: a 5 ms loop
    /// with a continuously easing gaze or a decaying pulse would otherwise
    /// dirty the canvas on every iteration and burn the whole raster budget on
    /// sub-pixel changes nobody can see.
    fn publish(&mut self) {
        let merged = self.persona.expression.unwrap_or(self.affect_expression);
        self.merged = merged;
        self.control.set_expression(merged);

        let visual = BallVisual {
            // The canvas pose scale is owned by the entrance; the personality
            // contributes the gaze and the pulse.
            scale_x: 1.0,
            scale_y: 1.0,
            look_x: quantize(self.persona.look_x, 0.5),
            look_y: quantize(self.persona.look_y, 0.5),
            pulse: quantize(self.persona.pulse, 0.125),
        };
        self.control.set_visual(visual);
    }

    fn push_haptic(&mut self, pattern: HapticPattern) {
        if self.pending_len < self.pending_haptics.len() {
            self.pending_haptics[self.pending_len] = Some(pattern);
            self.pending_len += 1;
        } else {
            // Keep the newest: a panic must not be lost behind a module beep.
            self.pending_haptics[self.pending_haptics.len() - 1] = Some(pattern);
        }
    }

    fn drain_haptics(&mut self) -> Option<HapticPattern> {
        let mut best: Option<HapticPattern> = None;
        for slot in self.pending_haptics.iter_mut().take(self.pending_len) {
            if let Some(pattern) = slot.take() {
                if best.map_or(true, |current| priority(pattern) > priority(current)) {
                    best = Some(pattern);
                }
            }
        }
        self.pending_len = 0;
        best
    }
}

/// Mirror of the haptic table's priority: Error > Panic > everything else.
fn priority(pattern: HapticPattern) -> u8 {
    match pattern {
        HapticPattern::Error => 3,
        HapticPattern::Panic => 2,
        _ => 1,
    }
}

/// Round to the nearest multiple of `step`.
fn quantize(value: f32, step: f32) -> f32 {
    if !value.is_finite() || step <= 0.0 {
        return 0.0;
    }
    let scaled = value / step;
    let rounded = if scaled >= 0.0 {
        (scaled + 0.5) as i32
    } else {
        (scaled - 0.5) as i32
    };
    rounded as f32 * step
}

/// Every event kind that can reach the affect engine.
pub fn is_ball_event(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Accel { .. }
            | EventKind::Gyro { .. }
            | EventKind::TouchPet
            | EventKind::TouchPoke
            | EventKind::LightLevel(_)
            | EventKind::Presence(_)
            | EventKind::KeyDown
            | EventKind::KeyUp
            | EventKind::ButtonShort
            | EventKind::ButtonLong
            | EventKind::ButtonDouble
            | EventKind::MagneticApproach
            | EventKind::MagneticContact
            | EventKind::MagneticRelease
            | EventKind::MagneticOrbit
            | EventKind::MagneticEdge { .. }
            | EventKind::ModuleInserted
            | EventKind::ModuleRemoved
            | EventKind::BatteryLevel(_)
            | EventKind::Activity(_)
    )
}

/// Look up the character skin the firmware should show.
pub fn default_skin() -> &'static skin::Skin {
    &skin::BLOB
}

#[cfg(test)]
mod tests {
    use super::*;
    use grok_ball::affect::{EventSource, ReactionReason};
    use grok_ball::personality::{SCREEN_CENTER_X, SCREEN_CENTER_Y};

    fn tap(runtime: &mut BallRuntime, at_ms: u64) {
        runtime.on_pointer(PointerSample {
            at_ms,
            x: SCREEN_CENTER_X,
            y: SCREEN_CENTER_Y,
            pressed: true,
        });
        runtime.on_pointer(PointerSample {
            at_ms: at_ms + 40,
            x: SCREEN_CENTER_X,
            y: SCREEN_CENTER_Y,
            pressed: false,
        });
    }

    #[test]
    fn every_event_kind_is_forwarded() {
        let kinds = [
            EventKind::Accel {
                ax_mg: 0,
                ay_mg: 0,
                az_mg: 1000,
            },
            EventKind::Gyro {
                gx_dps: 0,
                gy_dps: 0,
                gz_dps: 0,
            },
            EventKind::TouchPet,
            EventKind::TouchPoke,
            EventKind::LightLevel(50),
            EventKind::Presence(true),
            EventKind::KeyDown,
            EventKind::KeyUp,
            EventKind::ButtonShort,
            EventKind::ButtonLong,
            EventKind::ButtonDouble,
            EventKind::MagneticApproach,
            EventKind::MagneticContact,
            EventKind::MagneticRelease,
            EventKind::MagneticOrbit,
            EventKind::MagneticEdge { confidence: 80 },
            EventKind::ModuleInserted,
            EventKind::ModuleRemoved,
            EventKind::BatteryLevel(50),
            EventKind::Activity(BallExpression::Idle),
        ];
        for kind in kinds {
            assert!(is_ball_event(kind), "{kind:?} must reach the affect engine");
        }
    }

    #[test]
    fn the_fresh_runtime_shows_the_idle_face() {
        let control = BallControl::new(BallExpression::Idle);
        let runtime = BallRuntime::new(0, Rc::clone(&control));
        assert_eq!(runtime.expression(), BallExpression::Idle);
        assert_eq!(runtime.affect_state().reason, ReactionReason::Idle);
    }

    #[test]
    fn a_sensor_reaction_reaches_the_ball() {
        let control = BallControl::new(BallExpression::Idle);
        let mut runtime = BallRuntime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::Touch,
            kind: EventKind::TouchPoke,
        });
        assert_eq!(runtime.expression(), BallExpression::Puzzled);
        assert_eq!(runtime.affect_state().reason, ReactionReason::TouchPoke);
    }

    #[test]
    fn a_personality_reaction_outranks_an_affect_reaction() {
        let control = BallControl::new(BallExpression::Idle);
        let mut runtime = BallRuntime::new(0, Rc::clone(&control));

        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::Touch,
            kind: EventKind::TouchPoke,
        });
        assert_eq!(runtime.expression(), BallExpression::Puzzled);

        tap(&mut runtime, 1_100);
        runtime.tick(1_100 + 40 + 220);
        assert_eq!(
            runtime.expression(),
            BallExpression::Curious,
            "the tap answers with its own reaction"
        );

        // Both layers have expired: the resting face returns.
        runtime.tick(1_100 + 40 + 220 + 2_000);
        assert_eq!(runtime.expression(), BallExpression::Idle);
    }

    #[test]
    fn a_low_battery_face_is_the_resting_layer() {
        let control = BallControl::new(BallExpression::Idle);
        let mut runtime = BallRuntime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::Battery,
            kind: EventKind::BatteryLevel(10),
        });
        assert_eq!(runtime.expression(), BallExpression::Tired);
    }

    #[test]
    fn the_runtime_publishes_the_tap_pulse_and_gaze() {
        let control = BallControl::new(BallExpression::Idle);
        let mut runtime = BallRuntime::new(0, Rc::clone(&control));

        runtime.on_pointer(PointerSample {
            at_ms: 1_000,
            x: SCREEN_CENTER_X + 80,
            y: SCREEN_CENTER_Y,
            pressed: true,
        });
        runtime.tick(1_020);
        let visual = runtime.visual();
        assert!(visual.pulse > 0.0, "the press pulses");
        assert!(visual.look_x > 0.0, "the eyes glance toward the finger");
    }

    #[test]
    fn a_reaction_buzzes_once_and_not_again() {
        let control = BallControl::new(BallExpression::Idle);
        let mut runtime = BallRuntime::new(0, Rc::clone(&control));

        tap(&mut runtime, 1_000);
        assert_eq!(runtime.tick(1_100), None, "a single tap does not buzz");

        tap(&mut runtime, 1_400);
        assert_eq!(
            runtime.tick(1_400 + 40 + 380),
            Some(HapticPattern::Puzzled),
            "the second tap buzzes when its reaction starts"
        );
        assert_eq!(runtime.tick(1_400 + 40 + 400), None, "no repeat buzz");
    }

    #[test]
    fn the_touch_hold_off_suppresses_taps() {
        let control = BallControl::new(BallExpression::Idle);
        let mut runtime = BallRuntime::new(0, Rc::clone(&control));
        runtime.ignore_touch_until(3_000);

        // The runtime reports the discard, so the UI task does not tag a
        // latency sample for a touch that deliberately did nothing.
        let acted = runtime.on_pointer(PointerSample {
            at_ms: 1_000,
            x: SCREEN_CENTER_X,
            y: SCREEN_CENTER_Y,
            pressed: true,
        });
        assert!(!acted, "a held-off press is reported as not acted on");
        runtime.on_pointer(PointerSample {
            at_ms: 1_040,
            x: SCREEN_CENTER_X,
            y: SCREEN_CENTER_Y,
            pressed: false,
        });
        runtime.tick(1_500);
        assert_eq!(runtime.expression(), BallExpression::Idle);

        let acted = runtime.on_pointer(PointerSample {
            at_ms: 3_100,
            x: SCREEN_CENTER_X,
            y: SCREEN_CENTER_Y,
            pressed: true,
        });
        assert!(acted, "after the hold-off the press is real");
        runtime.on_pointer(PointerSample {
            at_ms: 3_140,
            x: SCREEN_CENTER_X,
            y: SCREEN_CENTER_Y,
            pressed: false,
        });
        runtime.tick(3_100 + 40 + 220);
        assert_eq!(runtime.expression(), BallExpression::Curious);
    }
}
