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
        allocator: &'static lua_engine::RawAllocator,
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
mod tests {
    use super::*;
    use {
        crate::companion::behavior::affect::EventKind,
        crate::companion::behavior::affect::EventSource,
        crate::companion::behavior::affect::ReactionReason,
    };
    use {
        crate::companion::behavior::personality::SCREEN_CENTER_X,
        crate::companion::behavior::personality::SCREEN_CENTER_Y,
    };

    #[test]
    fn bundled_bootstrap_retains_pre_mount_inputs_and_bad_selection_falls_back_warm() {
        let mut native = Runtime::new(0, FoxControl::new(Expression::Idle));
        let mut runtime = Runtime::new(0, FoxControl::new(Expression::Idle));
        runtime
            .enable_brain(
                crate::companion::brain::DEFAULT_SOURCE,
                0,
                &lua_engine::GLOBAL_ALLOCATOR,
                None,
            )
            .unwrap();
        runtime.ignore_touch_until(1000);
        native.ignore_touch_until(1000);
        // These arrive while USB/storage startup is pending. Module insertion
        // is an edge, and must not disappear into a newly created Idle engine.
        for event in [
            SensorEvent {
                at_ms: 10,
                source: EventSource::ModuleRight,
                kind: EventKind::ModuleInserted(
                    crate::companion::behavior::affect::ModuleKind::Interaction,
                ),
            },
            SensorEvent {
                at_ms: 20,
                source: EventSource::Battery,
                kind: EventKind::BatteryLevel(3),
            },
            SensorEvent {
                at_ms: 30,
                source: EventSource::Light,
                kind: EventKind::LightLevel(60),
            },
        ] {
            native.on_sensor(event);
            runtime.on_sensor(event);
        }
        assert_eq!(native.tick(100), runtime.tick(100));
        assert_eq!(runtime.expression(), native.expression());
        assert_ne!(runtime.expression(), Expression::Idle);
        // An absent persisted selection leaves this active VM untouched. Its
        // module sequence continues instead of restarting at storage-ready.
        assert_eq!(native.tick(1000), runtime.tick(1000));
        assert_eq!(runtime.expression(), native.expression());
        assert_eq!(native.tick(6000), runtime.tick(6000));
        assert_eq!(runtime.expression(), Expression::Dying);
        assert_eq!(runtime.expression(), native.expression());
        // A present but bad persisted source deliberately switches to the
        // native engines that consumed the very same early inputs.
        assert!(runtime
            .enable_brain(
                "error('bad persisted selection')",
                6100,
                &lua_engine::GLOBAL_ALLOCATOR,
                None
            )
            .is_err());
        assert!(!runtime.brain_active());
        assert_eq!(runtime.expression(), Expression::Dying);
        assert_eq!(native.tick(6200), runtime.tick(6200));
        assert_eq!(runtime.expression(), native.expression());
        runtime
            .reload_brain(
                "return {on_event=function()end,tick=function()fox.set_expression('Happy')end}",
                6300,
            )
            .unwrap();
        assert!(runtime.brain_active());
        assert_eq!(runtime.expression(), Expression::Happy);
    }

    #[test]
    fn native_urgent_reflexes_override_custom_brain_and_default_off_home_filter_matches() {
        let control = FoxControl::new(Expression::Idle);
        let mut custom = Runtime::new(0, Rc::clone(&control));
        custom.enable_brain("return {on_event=function()end,tick=function()fox.set_expression('Happy');fox.play('Charging',5000);haptic.play('Happy')end}",0,&lua_engine::GLOBAL_ALLOCATOR,None).unwrap();
        custom.on_sensor(SensorEvent {
            at_ms: 100,
            source: EventSource::System,
            kind: EventKind::Activity(Expression::Error),
        });
        assert_eq!(custom.expression(), Expression::Error);
        assert_eq!(control.play_pose(), None);
        assert_ne!(custom.tick(100), Some(HapticPattern::Happy));
        custom.on_sensor(SensorEvent {
            at_ms: 200,
            source: EventSource::System,
            kind: EventKind::Activity(Expression::Idle),
        });
        assert_eq!(custom.expression(), Expression::Happy);
        let mut native = Runtime::new(0, FoxControl::new(Expression::Idle));
        let mut lua = Runtime::new(0, FoxControl::new(Expression::Idle));
        lua.enable_brain(
            crate::companion::brain::DEFAULT_SOURCE,
            0,
            &lua_engine::GLOBAL_ALLOCATOR,
            None,
        )
        .unwrap();
        for kind in [
            EventKind::TouchPoke,
            EventKind::KeyDown,
            EventKind::Accel {
                ax_mg: 2000,
                ay_mg: 0,
                az_mg: 0,
            },
            EventKind::BatteryLevel(3),
        ] {
            if crate::shell::model::reaches_fox_off_home(&kind) {
                let event = SensorEvent {
                    at_ms: 100,
                    source: EventSource::Touch,
                    kind,
                };
                native.on_sensor(event);
                lua.on_sensor(event);
            }
        }
        assert_eq!(native.tick(500), lua.tick(500));
        assert_eq!(native.expression(), lua.expression());
    }

    #[test]
    fn resident_runtime_is_warm_and_recovers_after_bad_boot_and_runtime_failure() {
        let control = FoxControl::new(Expression::Idle);
        let mut native = Runtime::new(0, Rc::clone(&control));
        let other = FoxControl::new(Expression::Idle);
        let mut lua = Runtime::new(0, other);
        assert!(lua
            .enable_brain(
                "error('bad persisted source')",
                0,
                &lua_engine::GLOBAL_ALLOCATOR,
                None
            )
            .is_err());
        assert!(!lua.brain_active());
        assert!(lua.brain_error().is_some());
        lua.reload_brain(crate::companion::brain::DEFAULT_SOURCE, 0)
            .unwrap();
        let events = [
            SensorEvent {
                at_ms: 100,
                source: EventSource::System,
                kind: EventKind::Activity(Expression::Listening),
            },
            SensorEvent {
                at_ms: 300,
                source: EventSource::Touch,
                kind: EventKind::TouchPet,
            },
            SensorEvent {
                at_ms: 500,
                source: EventSource::Light,
                kind: EventKind::LightLevel(60),
            },
        ];
        for event in events {
            native.on_sensor(event);
            lua.on_sensor(event);
            assert_eq!(native.expression(), lua.expression());
            assert_eq!(native.tick(event.at_ms), lua.tick(event.at_ms));
        }
        lua.reload_brain("return {on_event=function()fox.set_expression('Angry');error('runtime fault')end,tick=function()fox.set_expression('Happy')end}",600).unwrap();
        assert_eq!(lua.expression(), Expression::Happy);
        let event = SensorEvent {
            at_ms: 700,
            source: EventSource::Battery,
            kind: EventKind::BatteryLevel(3),
        };
        native.on_sensor(event);
        lua.on_sensor(event);
        assert!(!lua.brain_active());
        assert_eq!(lua.expression(), native.expression());
        assert_eq!(lua.tick(10000), native.tick(10000));
        assert_eq!(lua.expression(), native.expression());
        lua.reload_brain("return {on_event=function()end,tick=function()fox.play('Charging',100);led_strip.set(1,255,0,0);haptic.play('Happy')end}",11000).unwrap();
        assert!(lua.brain_leds().is_some());
        lua.begin_shutdown();
        assert_eq!(lua.expression(), Expression::Sleep);
        assert!(lua.brain_leds().is_none());
        assert_eq!(lua.tick(12000), None);
        assert!(lua
            .reload_brain(crate::companion::brain::DEFAULT_SOURCE, 12000)
            .is_err());
    }

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
    fn shutdown_overrides_reactions_and_is_terminal_without_haptics() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 100,
            source: EventSource::ButtonPower,
            kind: EventKind::ButtonShort,
        });
        runtime.begin_shutdown();
        assert_eq!(runtime.expression(), Expression::Sleep);
        runtime.on_sensor(SensorEvent {
            at_ms: 200,
            source: EventSource::System,
            kind: EventKind::Activity(Expression::Error),
        });
        tap(&mut runtime, 300);
        runtime.on_interaction(500);
        assert_eq!(runtime.tick(10_000), None);
        assert_eq!(control.expression(), Expression::Sleep);
        assert_eq!(control.visual(), FoxVisual::default());
        runtime.begin_shutdown();
        assert_eq!(runtime.tick(20_000), None);
    }

    #[test]
    fn a_screen_tap_wakes_her_drowsy_instead_of_a_tap_reaction() {
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = Runtime::new(0, Rc::clone(&control));
        runtime.on_sensor(SensorEvent {
            at_ms: 0,
            source: EventSource::Light,
            kind: EventKind::LightLevel(5),
        });
        runtime.tick(60_000);
        assert_eq!(runtime.expression(), Expression::Sleep);

        tap(&mut runtime, 61_000);
        runtime.tick(61_300);
        assert_eq!(runtime.expression(), Expression::Tired);
        assert_eq!(runtime.affect_state().reason, ReactionReason::Wake);

        // A second tap while she is still drowsy makes her grumpy.
        tap(&mut runtime, 62_000);
        runtime.tick(62_100);
        assert_eq!(runtime.expression(), Expression::Angry);
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

        // A violent shake, five hard peaks inside a second: the affect engine
        // panics.
        for n in (0..5u64).rev() {
            accel(&mut runtime, 990 - n * 60, 1_000);
            accel(&mut runtime, 1_000 - n * 60, 2_500);
        }
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

        // A discarded touch must not request an immediate response frame.
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
