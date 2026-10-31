//! Ordered sensor and pointer drains on the existing UI loop.
use super::*;
pub(super) struct Drained {
    pub changed: bool,
}
pub(super) async fn sensors(app: &mut MosaicoApp, runtime: &mut Runtime, now_ms: u64) -> Drained {
    // ---- sensor events -> affect engine ----
    // Drained before the render so a shake that happened during the last
    // frame is part of this one. The channel is bounded and the producer
    // drops the oldest non-edge event, so this loop always terminates.

    let mut sensor_count = 0u32;
    let mut sensors_changed = false;
    while let Ok(event) = SENSOR_EVENTS.try_receive() {
        sensor_count = sensor_count.saturating_add(1);
        sensors_changed |= app.hub_mut().snapshot.observe(&event);
        // Off Home the fox is paused: the apps own the lights and the
        // motor, and only lasting state (battery, modules) reaches her.
        match event.kind {
            crate::companion::behavior::affect::EventKind::KeyDown => {
                lights::native_reflex(now_ms, 260)
            }
            crate::companion::behavior::affect::EventKind::ModuleInserted(_)
            | crate::companion::behavior::affect::EventKind::ModuleRemoved(_) => {
                lights::native_reflex(now_ms, 5000)
            }
            _ => {}
        }
        if app.page() == Page::Home || reaches_fox_off_home(&event.kind) {
            runtime.on_sensor(event);
        }
        // A FIFO batch may deliver 32 IMU events at once. Keep their
        // order, but let input I2C completion and HCI pumping run between
        // brain callbacks instead of waiting for the whole batch/render.
        embassy_futures::yield_now().await;
        if sensor_count >= SENSOR_DRAIN_MAX {
            break;
        }
    }
    if sensor_count >= SENSOR_DRAIN_MAX {
        warn!("ui_task: sensor drain hit {SENSOR_DRAIN_MAX} events in one loop");
    }

    Drained {
        changed: sensors_changed,
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
                        lvgl_port::set_pointer(sample.x, sample.y, sample.pressed);
                    } else {
                        lua::set_pointer(sample.x, sample.y, sample.pressed);
                    }
                } else {
                    lvgl_port::set_pointer(sample.x, sample.y, sample.pressed);
                }
            }
        }
        // A touch burst must likewise allow the hardware producers to
        // progress while every press/release still reaches its consumer.
        embassy_futures::yield_now().await;
    }
}
