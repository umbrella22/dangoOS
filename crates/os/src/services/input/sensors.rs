//! Sensor task.
//!
//! ```text
//! 20 ms Ticker
//!   ├─ every tick:    BMI270 FIFO drain (1-16 frames) -> Accel/Gyro events
//!   ├─ every 2 ticks: BMM150 data-ready read         -> magnetic edges
//!   ├─ every 2 ticks: Interaction LDR/PIR/KEY_L/KEY_R -> light/presence/key edges
//!   ├─ every 5 s:     BQ27220 state of charge        -> BatteryLevel event
//!   └─ every 250 ms:  left/right slot EEPROM probe   -> module insert/remove
//! ```
//!
//! The task owns the sensor *protocol*, not the bus: every transaction goes
//! through [`crate::platform::buses::shared_i2c`], which serialises I2C0 with the touch
//! task and enforces the 100 ms per-transaction timeout. The subboard EEPROM
//! and the optional BMM150 fallback use the separate I2C1 slot. No transaction
//! spans an await outside the driver helpers, so touch reads cannot be starved
//! by a long sensor burst.
//!
//! ## Queue policy
//!
//! [`SENSOR_EVENTS`] is the bounded `SensorEvent` channel (capacity 64) for
//! the UI/affect consumer. A full queue evicts the **oldest non-edge** event
//! and increments [`SENSOR_DROPS`]. Edge events (module insert/remove,
//! magnetic approach/contact/release/orbit/edge, key edges, presence edges,
//! button edges) are never evicted. If every queued event is an edge, the new
//! edge is explicitly counted in [`SENSOR_EDGE_DROPS`] and logged; it is never
//! lost silently.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::companion::behavior::mood_light::Side;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Ticker};
use log::{info, warn};
use {
    crate::companion::behavior::affect::EventKind, crate::companion::behavior::affect::EventSource,
    crate::companion::behavior::affect::SensorEvent,
};
use {
    crate::companion::behavior::magnet::MagReading,
    crate::companion::behavior::magnet::MagneticClassifier,
    crate::companion::behavior::magnet::MagneticEvent,
};

use crate::platform::drivers::battery::Bq27220;
use crate::services::feedback::lights;
use {
    crate::platform::drivers::imu, crate::platform::drivers::imu::Bmi270,
    crate::platform::drivers::imu::Bmm150, crate::platform::drivers::imu::ImuSample,
    crate::platform::drivers::imu::MagSample,
};
use {
    crate::platform::drivers::subboard, crate::platform::drivers::subboard::Slot,
    crate::platform::drivers::subboard::Subboard,
};

/// `SensorEvent` queue depth (RFC 0005 section 3.5).
/// Coherent hardware snapshot, copied by the Lua task. Never exposes a bus.
static LUA_HARDWARE: critical_section::Mutex<
    core::cell::RefCell<Option<crate::services::input::snapshot::HardwareSnapshot>>,
> = critical_section::Mutex::new(core::cell::RefCell::new(None));
pub fn lua_snapshot() -> crate::services::input::snapshot::HardwareSnapshot {
    critical_section::with(|cs| (*LUA_HARDWARE.borrow_ref(cs)).unwrap_or_default())
}
fn store_lua_snapshot(snapshot: crate::services::input::snapshot::HardwareSnapshot) {
    critical_section::with(|cs| *LUA_HARDWARE.borrow_ref_mut(cs) = Some(snapshot));
}

pub const SENSOR_QUEUE_CAPACITY: usize = 64;

/// `sensor_task` -> UI/affect consumer.
pub static SENSOR_EVENTS: Channel<CriticalSectionRawMutex, SensorEvent, SENSOR_QUEUE_CAPACITY> =
    Channel::new();

/// Non-edge events discarded because the queue was full. Never reset.
pub static SENSOR_DROPS: AtomicU32 = AtomicU32::new(0);

/// Edge events that could not be preserved because the queue held only edge
/// events. Expected to stay zero; reported separately so the acceptance run
/// can prove that no edge event was lost.
pub static SENSOR_EDGE_DROPS: AtomicU32 = AtomicU32::new(0);

/// 50 Hz base tick.
const TICK_MS: u64 = 20;
/// Wait after a module change before re-learning the magnetometer baselines,
/// so the hand that inserted it has moved away.
const MAG_RECALIBRATE_SETTLE_MS: u64 = 800;
/// BMI270 output period (50 Hz ODR), used to back-date FIFO samples.
const IMU_SAMPLE_MS: u64 = 20;
/// BMM150 / module-input polls every second tick (25 Hz).
const SLOW_DIVIDER: u32 = 2;
/// Battery state-of-charge poll period. The gauge integrates over seconds, so
/// 5 s is far more often than the value can change; a slower poll also keeps
/// the shared I2C0 bus free for touch.
const BATTERY_PERIOD_MS: u64 = 5_000;
#[embassy_executor::task]
pub async fn sensor_task(mut subboard: Subboard) {
    if !crate::services::power::battery_task::wait_boot().await {
        return;
    }
    info!(
        "sensor_task: IMU 50 Hz, BMM150 25 Hz poll, slot {} ms scan (queue={})",
        subboard::SCAN_PERIOD_MS,
        SENSOR_QUEUE_CAPACITY
    );

    // Sensor bring-up is asynchronous and never fatal: the task keeps the
    // slot scanner and the remaining devices running if one probe fails.
    let mut imu = Bmi270::probe().await;
    if let Some(device) = &mut imu {
        if let Err(error) = device.configure().await {
            warn!("BMI270 initialization failed: {error}");
            imu = None;
        }
    }

    let mut mags = Bmm150::probe_all().await;

    for slot in 0..mags.len() {
        let failed = match &mut mags[slot] {
            Some(device) => match device.configure().await {
                Ok(()) => false,
                Err(error) => {
                    warn!(
                        "BMM150[{}] @{} 0x{:02X} initialization failed: {error}",
                        slot,
                        device.bus().name(),
                        device.address()
                    );
                    true
                }
            },
            None => false,
        };
        if failed {
            mags[slot] = None;
        }
    }

    // BQ27220: read-only probe. A board without a gauge (or a gauge that does
    // not answer) simply keeps the normal resting face.
    let mut battery = Bq27220::probe().await;

    let mut hardware = crate::services::input::snapshot::HardwareSnapshot::default();
    hardware.available = u32::from(imu.is_some())
        | (u32::from(mags.iter().any(Option::is_some)) << 1)
        | (u32::from(battery.is_some()) << 2);
    let mut classifiers = [MagneticClassifier::new(), MagneticClassifier::new()];

    // A module carries its own magnets; re-learn the baseline once it sits.
    let mut mag_recalibrate_at: Option<u64> = None;
    let mut last_mag = [None::<MagSample>; 2];
    let mut ticker = Ticker::every(Duration::from_millis(TICK_MS));
    let mut tick: u32 = 0;

    let mut next_scan_ms = 0u64;

    let mut next_battery_ms = now_ms();

    let mut battery_errors: u32 = 0;

    let mut imu_batch = [ImuSample::default(); imu::FIFO_MAX_FRAMES];
    let mut last_imu_at = 0u64;
    loop {
        ticker.next().await;
        tick = tick.wrapping_add(1);

        // ---- IMU: drain the BMI270 FIFO (every sample at the sensor ODR) ----
        if let Some(device) = imu.as_mut() {
            let result = device.read_all(&mut imu_batch).await;
            if result.is_err() {
                hardware.valid &= !3;
            }
            if matches!(result, Ok(count) if count > 0) {
                match device.read_aux().await {
                    Ok((temperature, time, status)) => {
                        hardware.temperature = temperature as i32;
                        hardware.sensor_time = time;
                        hardware.imu_status = status as u32;
                        hardware.valid |= 12;
                    }
                    Err(_) => hardware.valid &= !12,
                }
            }
            match result {
                Ok(0) => {}
                Ok(count) => {
                    // The batch is oldest first and one ODR period apart.
                    // Back-date each sample, but never before the previous
                    // batch, so timestamps stay monotonic for the engine.
                    let now = now_ms();
                    for (index, sample) in imu_batch[..count].iter().enumerate() {
                        let age = (count - 1 - index) as u64 * IMU_SAMPLE_MS;
                        let at = now.saturating_sub(age).max(last_imu_at);
                        last_imu_at = at;
                        if let Some(raw) = sample.accel_raw {
                            hardware.accel = raw.map(i32::from);
                            hardware.valid |= 1;
                        }
                        if let Some(raw) = sample.gyro_raw {
                            hardware.gyro = raw.map(i32::from);
                            hardware.valid |= 2;
                        }
                        if let Some([ax_mg, ay_mg, az_mg]) = sample.accel_mg {
                            // A zero vector means the first conversion has not
                            // landed; it would look like a 1 g dynamic shock.
                            if ax_mg != 0 || ay_mg != 0 || az_mg != 0 {
                                publish(SensorEvent {
                                    at_ms: at,
                                    source: EventSource::Imu,
                                    kind: EventKind::Accel {
                                        ax_mg,
                                        ay_mg,
                                        az_mg,
                                    },
                                });
                            }
                        }
                        if let Some([gx_dps, gy_dps, gz_dps]) = sample.gyro_dps {
                            publish(SensorEvent {
                                at_ms: at,
                                source: EventSource::Imu,
                                kind: EventKind::Gyro {
                                    gx_dps,
                                    gy_dps,
                                    gz_dps,
                                },
                            });
                        }
                    }
                }
                Err(_) => {}
            }
        }

        // ---- BMM150: 25 Hz poll, one event only on a classified edge ----
        if tick % SLOW_DIVIDER == 1 {
            for (index, device) in mags.iter_mut().enumerate() {
                let Some(device) = device else {
                    continue;
                };
                match device.read().await {
                    Ok(Some(sample)) => {
                        let at = now_ms();
                        last_mag[index] = Some(sample);
                        let reading = MagReading {
                            x_ut: sample.x_ut,
                            y_ut: sample.y_ut,
                            magnitude_ut: sample.magnitude_ut,
                        };
                        let event = classifiers[index].update(&reading, at);
                        if index == 0 || last_mag[0].is_none() {
                            crate::services::input::snapshot::record_magnetic(
                                &mut hardware,
                                [sample.x_ut, sample.y_ut, sample.z_ut],
                                sample.status,
                                event,
                            );
                        }
                        match event {
                            Some(MagneticEvent::Approach { .. }) => publish(SensorEvent {
                                at_ms: at,
                                source: EventSource::Bmm150,
                                kind: EventKind::MagneticApproach,
                            }),
                            Some(MagneticEvent::Contact) => publish(SensorEvent {
                                at_ms: at,
                                source: EventSource::Bmm150,
                                kind: EventKind::MagneticContact,
                            }),
                            Some(MagneticEvent::Release) => publish(SensorEvent {
                                at_ms: at,
                                source: EventSource::Bmm150,
                                kind: EventKind::MagneticRelease,
                            }),
                            Some(MagneticEvent::Orbit { .. }) => publish(SensorEvent {
                                at_ms: at,
                                source: EventSource::Bmm150,
                                kind: EventKind::MagneticOrbit,
                            }),
                            Some(MagneticEvent::Edge { confidence }) => publish(SensorEvent {
                                at_ms: at,
                                source: EventSource::Bmm150,
                                kind: EventKind::MagneticEdge { confidence },
                            }),
                            None => {}
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {
                        if index == 0 || last_mag[0].is_none() {
                            crate::services::input::snapshot::invalidate_magnetic(&mut hardware);
                        }
                    }
                }
            }
        }

        hardware.button_board(subboard.active_slot().map(|slot| match slot {
            Slot::Left => [13, 12],
            Slot::Right => [11, 10],
        }));
        // ---- Interaction board inputs: 25 Hz, offset from the BMM poll ----
        if tick % SLOW_DIVIDER == 0 {
            if let Some(inputs) = subboard.poll_inputs() {
                let at = now_ms();
                hardware.valid |= 64;
                if let Some(pressed) = inputs.key_l_edge {
                    hardware.button_edge(0, pressed, at as u32);
                }
                if let Some(pressed) = inputs.key_r_edge {
                    hardware.button_edge(1, pressed, at as u32);
                }
                publish(SensorEvent {
                    at_ms: at,
                    source: EventSource::Light,
                    kind: EventKind::LightLevel(inputs.light_level),
                });
                lights::set_ambient(inputs.light_level);

                if let Some(pressed) = inputs.key_l_edge {
                    if pressed {
                        lights::spark(Side::Left);
                    }
                    publish(SensorEvent {
                        at_ms: at,
                        source: EventSource::KeyLeft,
                        kind: if pressed {
                            EventKind::KeyDown
                        } else {
                            EventKind::KeyUp
                        },
                    });
                }
                if let Some(pressed) = inputs.key_r_edge {
                    if pressed {
                        lights::spark(Side::Right);
                    }
                    publish(SensorEvent {
                        at_ms: at,
                        source: EventSource::KeyRight,
                        kind: if pressed {
                            EventKind::KeyDown
                        } else {
                            EventKind::KeyUp
                        },
                    });
                }
                if let Some(present) = inputs.pir_edge {
                    publish(SensorEvent {
                        at_ms: at,
                        source: EventSource::Pir,
                        kind: EventKind::Presence(present),
                    });
                }
            }
        }

        // ---- BQ27220 state of charge: one short read every 5 s ----
        let now = now_ms();
        if now >= next_battery_ms {
            if let Some(gauge) = &mut battery {
                match gauge.read().await {
                    Ok(sample) => {
                        publish(SensorEvent {
                            at_ms: now_ms(),
                            source: EventSource::Battery,
                            kind: EventKind::BatteryLevel(sample.percent),
                        });
                        hardware.voltage_mv = sample.voltage_mv as u32;
                        hardware.current_ma = sample.average_current_ma as i32;
                        hardware.soc = sample.percent as u32;
                        hardware.charging = u32::from(sample.average_current_ma > 0);
                        hardware.valid |= 32;
                        battery_errors = 0;
                    }
                    Err(error) => {
                        hardware.valid &= !32;
                        battery_errors = battery_errors.saturating_add(1);
                        // A gauge that fails a dozen consecutive reads is
                        // treated as absent rather than logging forever.
                        if battery_errors >= 12 {
                            warn!("BQ27220: disabling battery reporting after repeated errors ({error})");
                            battery = None;
                            hardware.available &= !4;
                        } else if battery_errors.is_multiple_of(4) {
                            warn!("BQ27220 read failed ({error}), attempt {battery_errors}");
                        }
                    }
                }
            }
            next_battery_ms = now.saturating_add(BATTERY_PERIOD_MS);
        }

        subboard.service_camera();
        // ---- Expansion slots: 250 ms presence scan ----
        if now >= next_scan_ms {
            for change in subboard.scan().await.into_iter().flatten() {
                mag_recalibrate_at = Some(now.saturating_add(MAG_RECALIBRATE_SETTLE_MS));
                publish(SensorEvent {
                    at_ms: now_ms(),
                    source: match change.slot {
                        Slot::Left => EventSource::ModuleLeft,
                        Slot::Right => EventSource::ModuleRight,
                    },
                    kind: if change.inserted {
                        EventKind::ModuleInserted(change.kind)
                    } else {
                        EventKind::ModuleRemoved(change.kind)
                    },
                });
            }
            let right = subboard.active_slot() == Some(Slot::Right);
            lights::set_attached(right);
            if right {
                hardware.available |= 16;
            } else {
                hardware.available &= !16;
            }
            hardware.button_board(subboard.active_slot().map(|slot| match slot {
                Slot::Left => [13, 12],
                Slot::Right => [11, 10],
            }));
            next_scan_ms = now.saturating_add(subboard::SCAN_PERIOD_MS);
        }

        if mag_recalibrate_at.is_some_and(|due| now >= due) {
            mag_recalibrate_at = None;
            for classifier in &mut classifiers {
                classifier.recalibrate();
            }
            hardware.magnet_state = 0;
        }
        store_lua_snapshot(hardware);
    }
}

/// Publish one event without blocking the sensor loop.
///
/// On a full queue the oldest non-edge event is evicted. Edge events are
/// rotated to the back of the FIFO while searching for a non-edge event, so no
/// edge is ever evicted. If all 64 entries are edge events, an incoming
/// non-edge event is dropped; an incoming edge event is counted in
/// [`SENSOR_EDGE_DROPS`] and logged. The rotation is synchronous, so no other
/// executor task observes an intermediate queue state.
pub(crate) fn publish(event: SensorEvent) {
    if SENSOR_EVENTS.try_send(event).is_ok() {
        return;
    }

    let mut evicted = false;
    for _ in 0..SENSOR_QUEUE_CAPACITY {
        let Ok(front) = SENSOR_EVENTS.try_receive() else {
            break;
        };
        if !is_edge(front.kind) {
            SENSOR_DROPS.fetch_add(1, Ordering::Relaxed);
            evicted = true;
            break;
        }
        // Preserve this edge at the back of the queue while we keep looking.
        if SENSOR_EVENTS.try_send(front).is_err() {
            // Unreachable: receiving `front` freed one slot. Count instead of
            // losing an edge silently if the channel contract ever changes.
            SENSOR_EDGE_DROPS.fetch_add(1, Ordering::Relaxed);
            warn!("sensor queue: failed to preserve an edge event");
            return;
        }
    }

    if !evicted {
        if is_edge(event.kind) {
            // Every one of the 64 entries is an edge. Keep them all and
            // account for the new event explicitly.
            SENSOR_EDGE_DROPS.fetch_add(1, Ordering::Relaxed);
            warn!("sensor queue: all {SENSOR_QUEUE_CAPACITY} entries are edge events; new edge dropped");
        } else {
            SENSOR_DROPS.fetch_add(1, Ordering::Relaxed);
        }
        return;
    }

    if SENSOR_EVENTS.try_send(event).is_err() {
        SENSOR_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Edge events must not be evicted by the overflow policy.
fn is_edge(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::TouchPet
            | EventKind::TouchPoke
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
            | EventKind::ModuleInserted(_)
            | EventKind::ModuleRemoved(_)
    )
}

fn now_ms() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis()
}
