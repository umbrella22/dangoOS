//! Runtime arbiter: sensor events, personality and the rendered expression.
//!
//! This module is the glue H2 was missing. It owns three things that used to be
//! either absent or spread across tasks:
//!
//! 1. **The affect engine** — `ui_task` drains [`crate::sensor_task::SENSOR_EVENTS`]
//!    into it, so shake/pet/poke/light/PIR/keys/module/magnet/BMM150 and the
//!    battery gauge actually drive expressions.
//! 2. **The personality engine** — the pointer stream goes here, not to LVGL
//!    events, so the tap ladder is decided with timestamps
//!    from the input task and rendered by the same code on host and device.
//! 3. **The merged frame** — the arbiter pushes one expression and one pose into
//!    the [`FoxControl`] per UI loop. Priority is (see [`merge_expression`]):
//!
//!    ```text
//!    urgent affect (Error, Panic)  >  personality reaction
//!                                  >  affect expression  >  resting expression
//!    ```
//!
//!    A tap answers with its own reaction even while the affect engine still
//!    shows a poke from before it, but a system error or the scream from a hard
//!    shake must survive the tap that follows it. The haptic arbitration below
//!    uses the same order, so what the user sees and feels always agree.
//!
//! Haptics from both engines are collected here; the caller forwards them to
//! [`crate::haptic::play`], which owns the 150 ms rate limit and the hardware.
//!
//! # Host testability
//!
//! Every input is a plain value (`SensorEvent`, `PointerSample`, `now_ms`) and
//! the output goes through the shared [`FoxControl`], so the merge rules are
//! unit-tested on the host without LVGL.

use alloc::rc::Rc;

use crate::behavior::affect::{AffectEngine, AffectState, HapticPattern, SensorEvent};
use crate::behavior::personality::{
    PersonaOutput, PersonaReaction, PersonalityEngine, PointerSample,
};
use crate::behavior::Expression;

use crate::ui::fox_widget::{FoxControl, FoxVisual};

/// The runtime state machine that turns sensors plus touch into one fox.
pub struct Runtime {
    affect: AffectEngine,
    personality: PersonalityEngine,
    control: Rc<FoxControl>,
    /// Expression the affect engine currently resolves to.
    affect_expression: Expression,
    /// Latest personality output.
    persona: PersonaOutput,
    /// Haptics produced since the last drain, newest last.
    pending_haptics: [Option<HapticPattern>; 2],
    /// Entry count for `pending_haptics`.
    pending_len: usize,
    /// The merged expression the control currently shows.
    merged: Expression,
}

impl Runtime {
    /// Create the runtime at `now_ms`.
    pub fn new(now_ms: u64, control: Rc<FoxControl>) -> Self {
        let mut runtime = Self {
            affect: AffectEngine::new(now_ms),
            personality: PersonalityEngine::new(now_ms),
            control,
            affect_expression: Expression::Idle,
            persona: PersonaOutput::IDLE,
            pending_haptics: [None; 2],
            pending_len: 0,
            merged: Expression::Idle,
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
    pub fn expression(&self) -> Expression {
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
    pub fn visual(&self) -> FoxVisual {
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
    /// `FoxControl::set_expression` and `set_visual` are change-gated, so this
    /// is free when nothing moved. The pose is quantized first: a 5 ms loop
    /// with a continuously easing gaze or a decaying pulse would otherwise
    /// dirty the canvas on every iteration and burn the whole raster budget on
    /// sub-pixel changes nobody can see.
    fn publish(&mut self) {
        let merged = merge_expression(self.persona.expression, self.affect_expression);
        self.merged = merged;
        self.control.set_expression(merged);

        let visual = FoxVisual {
            // The canvas pose scale is owned by the entrance; the personality
            // contributes the gaze and the pulse.
            scale_x: 1.0,
            scale_y: 1.0,
            look_x: quantize(self.persona.look_x, 0.5),
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
                if best.map_or(true, |current| pattern.priority() > current.priority()) {
                    best = Some(pattern);
                }
            }
        }
        self.pending_len = 0;
        best
    }
}

/// Affect expressions that no touch reaction may cover.
///
/// `Error` is a system state the user must see; `Panic` is the shake scream
/// whose haptic already outranks every touch buzz in [`HapticPattern::priority`].
fn is_urgent(expression: Expression) -> bool {
    matches!(expression, Expression::Error | Expression::Panic)
}

/// Merge the personality reaction with the affect expression.
///
/// Urgent affect states win; otherwise an active personality reaction wins;
/// otherwise the affect engine's (possibly resting) expression shows.
pub fn merge_expression(persona: Option<Expression>, affect: Expression) -> Expression {
    if is_urgent(affect) {
        return affect;
    }
    persona.unwrap_or(affect)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::affect::{EventKind, EventSource, ReactionReason};
    use crate::behavior::personality::{SCREEN_CENTER_X, SCREEN_CENTER_Y};

    fn tap(runtime: &mut Runtime, at_ms: u64) {
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
    fn the_fresh_runtime_shows_the_idle_face() {
        let control = FoxControl::new(Expression::Idle);
        let runtime = Runtime::new(0, Rc::clone(&control));
        assert_eq!(runtime.expression(), Expression::Idle);
        assert_eq!(runtime.affect_state().reason, ReactionReason::Idle);
    }

    #[test]
    fn a_sensor_reaction_reaches_the_fox() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::Touch,
            kind: EventKind::TouchPoke,
        });
        assert_eq!(runtime.expression(), Expression::Puzzled);
        assert_eq!(runtime.affect_state().reason, ReactionReason::TouchPoke);
    }

    #[test]
    fn a_personality_reaction_outranks_an_affect_reaction() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));

        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::Touch,
            kind: EventKind::TouchPoke,
        });
        assert_eq!(runtime.expression(), Expression::Puzzled);

        tap(&mut runtime, 1_100);
        runtime.tick(1_100 + 40 + 220);
        assert_eq!(
            runtime.expression(),
            Expression::Curious,
            "the tap answers with its own reaction"
        );

        // Both layers have expired: the resting face returns.
        runtime.tick(1_100 + 40 + 220 + 2_000);
        assert_eq!(runtime.expression(), Expression::Idle);
    }

    fn accel(runtime: &mut Runtime, at_ms: u64, az_mg: i16) {
        runtime.on_sensor(SensorEvent {
            at_ms,
            source: EventSource::Imu,
            kind: EventKind::Accel {
                ax_mg: 0,
                ay_mg: 0,
                az_mg,
            },
        });
    }

    #[test]
    fn a_shake_panic_survives_the_tap_that_follows_it() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));

        // Two large peaks within 200 ms: the affect engine panics.
        accel(&mut runtime, 950, 2_500);
        accel(&mut runtime, 1_000, 2_500);
        runtime.tick(1_000);
        assert_eq!(runtime.expression(), Expression::Panic);

        // A tap during the scream must not cover it.
        tap(&mut runtime, 1_100);
        runtime.tick(1_100 + 40 + 220);
        assert_eq!(
            runtime.expression(),
            Expression::Panic,
            "an urgent affect state outranks a touch reaction"
        );
    }

    #[test]
    fn an_error_survives_a_tap() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::System,
            kind: EventKind::Activity(Expression::Error),
        });
        runtime.tick(1_000);
        assert_eq!(runtime.expression(), Expression::Error);

        tap(&mut runtime, 1_100);
        runtime.tick(1_100 + 40 + 220);
        assert_eq!(runtime.expression(), Expression::Error);
    }

    #[test]
    fn merge_order_matches_the_haptic_order() {
        use Expression as E;
        assert_eq!(merge_expression(Some(E::Curious), E::Error), E::Error);
        assert_eq!(merge_expression(Some(E::Angry), E::Panic), E::Panic);
        assert_eq!(merge_expression(Some(E::Curious), E::Puzzled), E::Curious);
        assert_eq!(merge_expression(None, E::Tired), E::Tired);
    }

    #[test]
    fn a_low_battery_face_is_the_resting_layer() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 1_000,
            source: EventSource::Battery,
            kind: EventKind::BatteryLevel(10),
        });
        assert_eq!(runtime.expression(), Expression::Tired);
    }

    #[test]
    fn the_runtime_publishes_the_tap_pulse_and_gaze() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));

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
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));

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
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
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
        assert_eq!(runtime.expression(), Expression::Idle);

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
        assert_eq!(runtime.expression(), Expression::Curious);
    }
}
