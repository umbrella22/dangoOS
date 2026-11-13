//! Ordered sensor and pointer drains on the existing UI loop.
use super::*;
pub(super) struct Drained {
    pub changed: bool,
    #[cfg(feature = "display-metrics")]
    pub events: u32,
    #[cfg(feature = "display-metrics")]
    pub work_us: u64,
    #[cfg(feature = "display-metrics")]
    pub yield_us: u64,
}
pub(super) async fn sensors(app: &mut MosaicoApp, runtime: &mut Runtime) -> Drained {
    // ---- sensor events -> affect engine ----
    // Drained before the render so a shake that happened during the last
    // frame is part of this one. The channel is bounded and the producer
    // drops the oldest non-edge event, so this loop always terminates.

    let mut sensor_count = 0u32;
    let mut sensors_changed = false;
    #[cfg(feature = "display-metrics")]
    let (mut work_us, mut yield_us) = (0, 0);
    while let Ok(event) = SENSOR_EVENTS.try_receive() {
        #[cfg(feature = "display-metrics")]
        let work_started = now_us();
        sensor_count = sensor_count.saturating_add(1);
        sensors_changed |= app.hub_mut().snapshot.observe(&event);
        // Off Home the fox is paused: the apps own the lights and the
        // motor, and only lasting state (battery, modules) reaches her.
        // AI short/double gestures execute here on every route. Exclude them
        // from the behavior runtime so one gesture issues one system action.
        use crate::companion::behavior::affect::{EventKind, EventSource};
        let ai_key = event.source == EventSource::ButtonPower
            && matches!(event.kind, EventKind::ButtonShort | EventKind::ButtonDouble);
        if ai_key {
            if event.kind == EventKind::ButtonShort {
                app.go(Page::App(AppId::Voice));
                app.update(AppMsg::VoiceToggle);
            } else {
                app.update(AppMsg::VoiceCancel);
            }
            sensors_changed = true;
        }
        if !ai_key && (app.page() == Page::Home || reaches_fox_off_home(&event.kind)) {
            runtime.on_sensor(event);
        }
        // A FIFO batch may deliver 32 IMU events at once. Keep their
        // order, but let input I2C completion and HCI pumping run between
        // sensor callbacks instead of waiting for the whole batch/render.
        #[cfg(feature = "display-metrics")]
        let yield_started = now_us();
        #[cfg(feature = "display-metrics")]
        {
            work_us += yield_started.saturating_sub(work_started);
        }
        embassy_futures::yield_now().await;
        #[cfg(feature = "display-metrics")]
        {
            yield_us += now_us().saturating_sub(yield_started);
        }
        if sensor_count >= SENSOR_DRAIN_MAX {
            break;
        }
    }
    if sensor_count >= SENSOR_DRAIN_MAX {
        warn!("ui_task: sensor drain hit {SENSOR_DRAIN_MAX} events in one loop");
    }

    Drained {
        changed: sensors_changed,
        #[cfg(feature = "display-metrics")]
        events: sensor_count,
        #[cfg(feature = "display-metrics")]
        work_us,
        #[cfg(feature = "display-metrics")]
        yield_us,
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn pointer(
    app: &MosaicoApp,
    runtime: &mut Runtime,
    ui: &mut Ui<AppMsg>,
    gestures: &mut EdgeGestures,
    press_state: &mut bool,
    pending_touch: &mut bool,
    sliding: bool,
    native_active: bool,
    now_ms: u64,
) {
    // ---- pointer queue -> personality engine ----
    // Every event is delivered in order: collapsing would swallow the press
    // edge of a tap that starts and ends inside one 5 ms loop.
    // Edge swipes go to the shell; the rest to the fox (Home) or LVGL.
    // Yielding lets producers refill the channel, so retain a finite
    // per-loop bound even while a finger keeps producing new samples.
    for _ in 0..INPUT_CAPACITY {
        let Ok(event) = INPUT.try_receive() else {
            break;
        };
        crate::platform::runtime::frequency::device::interaction();

        let sample = PointerSample {
            at_ms: (event.at_us / 1000).max(now_ms),
            x: event.x as i32,
            y: event.y as i32,
            pressed: event.pressed,
        };
        match gestures.feed(sample) {
            TouchRoute::Gesture(gesture) => {
                // Swiping between pages counts as using her: stay awake.
                runtime.on_interaction(sample.at_ms);
                if let Err(error) = ui.post(AppMsg::Gesture(gesture)) {
                    warn!("ui_task: gesture post rejected: {error}");
                }
            }
            TouchRoute::Held => {}
            TouchRoute::Pass if app.page() == Page::Home => {
                let was_pressed = *press_state;
                *press_state = event.pressed;
                // Only accepted presses expedite a response frame; startup
                // hold-off must not bypass the normal display cadence.
                let acted = runtime.on_pointer(sample);
                if event.pressed && !was_pressed && acted {
                    *pending_touch = true;
                }
            }
            // Touches during a frames.slide are dropped; LVGL resumes clean.
            TouchRoute::Pass if sliding => *press_state = event.pressed,
            TouchRoute::Pass => {
                if event.pressed && !*press_state {
                    // Using an app keeps her awake, without a wake face.
                    runtime.on_interaction(sample.at_ms);
                }
                *press_state = event.pressed;
                if is_lua_page(app.page()) {
                    if native_active {
                        lvgl_port::set_pointer_at(sample.x, sample.y, sample.pressed, event.at_us);
                    } else {
                        lua::set_pointer(sample.x, sample.y, sample.pressed);
                    }
                } else {
                    lvgl_port::set_pointer_at(sample.x, sample.y, sample.pressed, event.at_us);
                }
            }
        }
        // A touch burst must likewise allow the hardware producers to
        // progress while every press/release still reaches its consumer.
        embassy_futures::yield_now().await;
    }
}
