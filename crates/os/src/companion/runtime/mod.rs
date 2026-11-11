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

use crate::companion::behavior::Expression;
use {
    crate::companion::behavior::affect::AffectEngine,
    crate::companion::behavior::affect::AffectState,
    crate::companion::behavior::affect::HapticPattern,
    crate::companion::behavior::affect::SensorEvent,
};
use {
    crate::companion::behavior::personality::PersonaOutput,
    crate::companion::behavior::personality::PersonaReaction,
    crate::companion::behavior::personality::PersonalityEngine,
    crate::companion::behavior::personality::PointerSample,
};

use {crate::companion::widget::FoxControl, crate::companion::widget::FoxVisual};

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
    /// Whether the last pointer sample was a press (to find press edges).
    pointer_down: bool,
    shutdown: bool,
    brain: Option<crate::companion::brain::Controller>,
    brain_haptic: Option<HapticPattern>,
    brain_command: Option<crate::companion::behavior::affect::SystemCommand>,
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
            pointer_down: false,
            shutdown: false,
            brain: None,
            brain_haptic: None,
            brain_command: None,
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
        if let Some(b) = self.brain.as_mut() {
            b.ignore_touch(until_ms);
        }
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

    /// Terminal power-off presentation overrides every reaction and suppresses
    /// haptics. Hardware power is cut only after the plane acknowledges Sleep.
    pub fn begin_shutdown(&mut self) {
        self.shutdown = true;
        self.brain = None;
        self.brain_haptic = None;
        self.brain_command = None;
        self.pending_len = 0;
        self.pending_haptics = [None; 2];
        self.publish();
    }

    /// Apply one sensor event.
    pub fn on_sensor(&mut self, event: SensorEvent) {
        if self.shutdown {
            return;
        }
        if let Some(b) = self.brain.as_mut() {
            b.event(event);
        }
        self.collect_brain_effects();
        let at = event.at_ms;
        self.affect.on_event(event);
        self.refresh_affect(at);
        self.publish();
    }

    /// Apply one pointer sample from the input task.
    ///
    /// Returns `false` when the sample was discarded by the boot hold-off. The
    /// caller uses that to expedite only touches with a visible response.
    pub fn on_pointer(&mut self, sample: PointerSample) -> bool {
        if self.shutdown {
            return false;
        }
        let brain_accepted = self
            .brain
            .as_mut()
            .filter(|b| b.active())
            .map(|b| b.pointer(sample));
        self.collect_brain_effects();
        let at = sample.at_ms;
        let press = sample.pressed && !self.pointer_down;
        self.pointer_down = sample.pressed;
        // Every press is an interaction for the sleep logic. A press that
        // wakes her up shows the wake face instead of a tap reaction; the
        // rest of that touch is ignored by the personality.
        if press && self.affect.on_screen_touch(at) {
            self.personality.ignore_until(
                at.saturating_add(crate::companion::behavior::affect::WAKE_DROWSY_MS),
            );
            self.refresh_affect(at);
            self.publish();
            return brain_accepted.unwrap_or(true);
        }
        if !self.personality.accepts(at) {
            self.publish();
            return brain_accepted.unwrap_or(false);
        }
        let mut output = self.personality.on_pointer(sample);
        if let Some(pattern) = output.haptic.take() {
            self.push_haptic(pattern);
        }
        self.persona = output;
        self.refresh_affect(at);
        self.publish();
        brain_accepted.unwrap_or(true)
    }

    /// The user is busy in a hub app: keep the fox awake (no wake face).
    pub fn on_interaction(&mut self, now_ms: u64) {
        // The finger may have left the fox's page: the next press on it is
        // a fresh press edge.
        if self.shutdown {
            return;
        }
        if let Some(b) = self.brain.as_mut() {
            b.interaction(now_ms);
        }
        self.collect_brain_effects();
        self.pointer_down = false;
        self.affect.on_interaction(now_ms);
        self.refresh_affect(now_ms);
        self.publish();
    }

    /// Advance timers and merge the layers. Call once per UI loop.
    ///
    /// Returns the one-shot haptic requested by this tick, if any: a
    /// personality reaction is more immediate than an affect reaction, so it
    /// wins when both fire together.
    pub fn tick(&mut self, now_ms: u64) -> Option<HapticPattern> {
        if self.shutdown {
            self.publish();
            return None;
        }
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

        if let Some(b) = self.brain.as_mut() {
            b.tick(now_ms);
        }
        self.collect_brain_effects();
        self.publish();
        let native = self.drain_haptics();
        if self.native_urgent() {
            self.brain_haptic = None;
            native
        } else if self.brain_active() {
            self.brain_haptic.take()
        } else {
            self.brain_haptic = None;
            self.brain_command = None;
            native
        }
    }

    pub fn enable_brain(
        &mut self,
        source: &str,
        now: u64,
        allocator: &'static app_host::RawAllocator,
        clock: Option<fn() -> u64>,
    ) -> Result<(), alloc::string::String> {
        let candidate =
            match crate::companion::brain::Controller::new(source, now, allocator, clock) {
                Ok(candidate) => candidate,
                Err(error) => {
                    self.brain = Some(crate::companion::brain::Controller::inactive(
                        source,
                        allocator,
                        clock,
                        error.clone(),
                    ));
                    self.publish();
                    return Err(error);
                }
            };
        self.brain = Some(candidate);
        self.brain_haptic = None;
        self.brain_command = None;
        self.collect_brain_effects();
        self.publish();
        Ok(())
    }
    pub fn reload_brain(&mut self, source: &str, now: u64) -> Result<(), alloc::string::String> {
        self.reload_brain_transaction(source, now, || Ok(()))
    }
    pub fn reload_brain_transaction(
        &mut self,
        source: &str,
        now: u64,
        commit: impl FnOnce() -> Result<(), alloc::string::String>,
    ) -> Result<(), alloc::string::String> {
        if self.shutdown {
            return Err("shutdown in progress".into());
        }
        self.brain
            .as_mut()
            .ok_or_else(|| alloc::string::String::from("brain not configured"))?
            .replace_with_commit(source, now, commit)?;
        self.brain_haptic = None;
        self.brain_command = None;
        self.collect_brain_effects();
        self.publish();
        Ok(())
    }
    fn native_urgent(&self) -> bool {
        matches!(
            self.affect_expression,
            Expression::Error | Expression::Panic
        )
    }
    pub async fn reload_brain_transaction_async(
        &mut self,
        source: &str,
        now: u64,
        commit: impl AsyncFnOnce() -> Result<(), alloc::string::String>,
    ) -> Result<(), alloc::string::String> {
        if self.shutdown {
            return Err("shutdown in progress".into());
        }
        self.brain
            .as_mut()
            .ok_or_else(|| alloc::string::String::from("brain not configured"))?
            .replace_with_commit_async(source, now, commit)
            .await?;
        self.brain_haptic = None;
        self.brain_command = None;
        self.collect_brain_effects();
        self.publish();
        Ok(())
    }
    pub fn native_expression(&self) -> Expression {
        if self.shutdown {
            Expression::Sleep
        } else {
            merge_expression(self.persona.expression, self.affect_expression)
        }
    }
    pub fn brain_source(&self) -> Option<&str> {
        self.brain.as_ref().map(|b| b.source())
    }
    pub fn brain_active(&self) -> bool {
        self.brain.as_ref().is_some_and(|b| b.active())
    }
    pub fn brain_error(&self) -> Option<&str> {
        self.brain.as_ref().and_then(|b| b.error())
    }
    pub fn brain_leds(&mut self) -> Option<[[u8; 3]; 6]> {
        if self.shutdown {
            None
        } else {
            self.brain
                .as_mut()
                .and_then(|b| b.output())
                .and_then(|o| o.leds)
        }
    }
    pub fn take_brain_command(
        &mut self,
    ) -> Option<crate::companion::behavior::affect::SystemCommand> {
        if self.brain_active() {
            self.brain_command.take()
        } else {
            self.brain_command = None;
            None
        }
    }
    fn collect_brain_effects(&mut self) {
        if let Some(o) = self.brain.as_mut().and_then(|b| b.output()) {
            if let Some(h) = o.haptic {
                if self
                    .brain_haptic
                    .is_none_or(|old| h.priority() > old.priority())
                {
                    self.brain_haptic = Some(h);
                }
            }
            if let Some(c) = o.command {
                self.brain_command = Some(c);
            }
        }
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
        let brain = self.brain.as_mut().and_then(|b| b.output());
        self.control
            .set_play_pose(if self.shutdown || self.native_urgent() {
                None
            } else {
                brain.and_then(|o| o.play.map(|(p, _)| p))
            });
        if !self.shutdown && !self.native_urgent() {
            if let Some(o) = brain {
                self.merged = o.expression;
                self.control.set_expression(o.expression);
                self.control.set_visual(FoxVisual {
                    look_x: quantize(o.visual.look_x, 0.5),
                    pulse: quantize(o.visual.pulse, 0.125),
                    pet: quantize(o.visual.pet, 0.0625),
                    lean_x: quantize(o.visual.lean_x, 1.),
                    ..o.visual
                });
                return;
            }
        }
        let merged = if self.shutdown {
            Expression::Sleep
        } else {
            merge_expression(self.persona.expression, self.affect_expression)
        };
        self.merged = merged;
        self.control.set_expression(merged);

        let visual = FoxVisual {
            // The canvas pose scale is owned by the entrance; the personality
            // contributes the gaze and the pulse.
            scale_x: 1.0,
            scale_y: 1.0,
            look_x: quantize(self.persona.look_x, 0.5),
            pulse: quantize(self.persona.pulse, 0.125),
            pet: quantize(self.persona.pet, 0.0625),
            lean_x: quantize(self.persona.lean_x, 1.0),
        };
        self.control.set_visual(if self.shutdown {
            FoxVisual::default()
        } else {
            visual
        });
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
mod tests;
