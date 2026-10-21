//! R1.5 sensor task.
//!
//! ```text
//! startup:          raw I2C scan of I2C0 + I2C1 -> bus/address map
//! 20 ms Ticker
//!   ├─ every tick:    BMI270 status+accel+gyro burst -> Accel/Gyro events
//!   ├─ every 2 ticks: BMM150 data-ready read         -> magnetic edges
//!   ├─ every 2 ticks: Interaction LDR/PIR/KEY_L/KEY_R -> light/presence/key edges
//!   ├─ every 5 s:     BQ27220 state of charge        -> BatteryLevel event
//!   └─ every 250 ms:  right-slot EEPROM probe        -> module insert/remove
//! ```
//!
//! The task owns the sensor *protocol*, not the bus: every transaction goes
//! through [`crate::drivers::shared_i2c`], which serialises I2C0 with the touch
//! task and enforces the 100 ms per-transaction timeout. The subboard EEPROM
//! and the optional BMM150 fallback use the separate I2C1 slot. No transaction
//! spans an await outside the driver helpers, so touch reads cannot be starved
//! by a long sensor burst.
//!
//! ## IMU rate accounting
//!
//! The scheduler is a single cooperative executor shared with the LVGL/display
//! tasks. If a UI frame runs long, `Ticker::next` fires late and this task
//! cannot sample at 50 Hz. The stats line therefore reports:
//!
//! - `imu_read` / `imax`: the last and maximum BMI270 burst duration,
//! - `late` / `lmax`: the last and maximum ticker lateness,
//! - `miss`: whole 20 ms periods lost to lateness.
//!
//! The rate is never padded or masked: a 25 Hz stream means the loop really
//! ran 25 times, not that samples were synthesised.
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

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Ticker};
use grok_ball::affect::{EventKind, EventSource, SensorEvent};
use log::{info, warn};

use crate::drivers::battery::Bq27220;
use crate::drivers::imu::{self, Bmi270, Bmm150, MagSample, MagneticClassifier, MagneticEvent};
use crate::drivers::module::{ModuleWatcher, ScanEvent};

/// `SensorEvent` queue depth (RFC 0005 section 3.5).
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
/// BMM150 / module-input polls every second tick (25 Hz).
const SLOW_DIVIDER: u32 = 2;
/// Battery state-of-charge poll period. The gauge integrates over seconds, so
/// 5 s is far more often than the value can change; a slower poll also keeps
/// the shared I2C0 bus free for touch.
const BATTERY_PERIOD_MS: u64 = 5_000;
/// Stats period.
const STATS_PERIOD_MS: u64 = 1_000;
/// The persistent `[scan]` summary is repeated so a USB CDC host that attaches
/// after boot still sees the bus/address result.
const SCAN_SUMMARY_PERIOD_MS: u64 = 10_000;

#[embassy_executor::task]
pub async fn sensor_task(mut module: ModuleWatcher) {
    info!(
        "sensor_task: IMU 50 Hz, BMM150 25 Hz poll, module {} ms scan (queue={})",
        crate::drivers::module::SCAN_PERIOD_MS,
        SENSOR_QUEUE_CAPACITY
    );

    // Definitive bus map before any driver probe: scan I2C0 and I2C1 for every
    // ACKing 7-bit address. This distinguishes "BMM150 on the other bus" from
    // "not populated / not powered", which the address probes alone cannot.
    imu::scan_buses().await;

    // Sensor bring-up is asynchronous and never fatal: the task keeps the
    // module watcher and the remaining devices running if one probe fails.
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

    // Persist the scan/probe result in statics and log it once now. The same
    // summary is repeated every SCAN_SUMMARY_PERIOD_MS, so it is readable
    // after the USB CDC host attaches later.
    imu::log_scan_summary();

    // BQ27220: read-only probe. A board without a gauge (or a gauge that does
    // not answer) simply keeps the normal resting face.
    let mut battery = Bq27220::probe().await;

    let mut classifiers = [MagneticClassifier::new(), MagneticClassifier::new()];
    let mut last_mag = [None::<MagSample>; 2];
    let mut ticker = Ticker::every(Duration::from_millis(TICK_MS));
    let mut tick: u32 = 0;

    let mut next_scan_ms = 0u64;
    let mut next_stats_ms = now_ms().saturating_add(STATS_PERIOD_MS);
    let mut next_scan_summary_ms = now_ms().saturating_add(SCAN_SUMMARY_PERIOD_MS);
    let mut next_battery_ms = now_ms();

    let mut imu_samples: u32 = 0;
    let mut imu_skips: u32 = 0;
    let mut imu_errors: u32 = 0;
    let mut bmm_samples: u32 = 0;
    let mut bmm_empty: u32 = 0;
    let mut bmm_errors: u32 = 0;
    let mut last_battery = None;
    let mut battery_errors: u32 = 0;

    let mut last_imu_samples = 0u32;
    let mut last_bmm_samples = 0u32;

    // IMU rate accounting (see the module docs). `next_tick_us` follows the
    // 20 ms grid; every late wake-up is measured, never hidden.
    let tick_period_us = TICK_MS * 1_000;
    let mut next_tick_us = now_us().saturating_add(tick_period_us);
    let mut late_us: u64;
    let mut late_max_us = 0u64;
    let mut missed_ticks = 0u32;
    let mut tick_late_active = false;
    let mut imu_read_us = 0u64;
    let mut imu_read_max_us = 0u64;

    loop {
        ticker.next().await;
        let loop_start_us = now_us();
        late_us = loop_start_us.saturating_sub(next_tick_us);
        if late_us > late_max_us {
            late_max_us = late_us;
        }
        if late_us >= tick_period_us {
            // Count one stall episode, not every catch-up iteration: `Ticker`
            // fires immediately until it is back on the grid.
            if !tick_late_active {
                missed_ticks = missed_ticks.saturating_add((late_us / tick_period_us) as u32);
                tick_late_active = true;
            }
        } else {
            tick_late_active = false;
        }
        // Follow the ideal grid exactly like `Ticker::expires_at`; no resync,
        // so the reported lateness stays comparable across iterations.
        next_tick_us = next_tick_us.saturating_add(tick_period_us);
        tick = tick.wrapping_add(1);

        // ---- IMU: one 21-byte burst at 50 Hz ----
        if let Some(device) = &mut imu {
            let read_started_us = now_us();
            let result = device.read().await;
            imu_read_us = now_us().saturating_sub(read_started_us);
            if imu_read_us > imu_read_max_us {
                imu_read_max_us = imu_read_us;
            }
            match result {
                Ok(Some(sample)) => {
                    let at = now_ms();
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
                            imu_samples = imu_samples.saturating_add(1);
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
                Ok(None) => {
                    imu_skips = imu_skips.saturating_add(1);
                }
                Err(_) => {
                    imu_errors = imu_errors.saturating_add(1);
                }
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
                        bmm_samples = bmm_samples.saturating_add(1);
                        let at = now_ms();
                        last_mag[index] = Some(sample);
                        match classifiers[index].update(&sample, at) {
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
                    Ok(None) => bmm_empty = bmm_empty.saturating_add(1),
                    Err(_) => bmm_errors = bmm_errors.saturating_add(1),
                }
            }
        }

        // ---- Interaction module inputs: 25 Hz, offset from the BMM poll ----
        if tick % SLOW_DIVIDER == 0 {
            if let Some(inputs) = module.poll_inputs() {
                let at = now_ms();
                publish(SensorEvent {
                    at_ms: at,
                    source: EventSource::Light,
                    kind: EventKind::LightLevel(inputs.light_level),
                });

                if let Some(pressed) = inputs.key_l_edge {
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
                        last_battery = Some(sample);
                        battery_errors = 0;
                    }
                    Err(error) => {
                        battery_errors = battery_errors.saturating_add(1);
                        // A gauge that fails a dozen consecutive reads is
                        // treated as absent rather than logging forever.
                        if battery_errors >= 12 {
                            warn!("BQ27220: disabling battery reporting after repeated errors ({error})");
                            battery = None;
                        } else if battery_errors.is_multiple_of(4) {
                            warn!("BQ27220 read failed ({error}), attempt {battery_errors}");
                        }
                    }
                }
            }
            next_battery_ms = now.saturating_add(BATTERY_PERIOD_MS);
        }

        // ---- Right-slot module: 250 ms presence scan ----
        if now >= next_scan_ms {
            match module.scan().await {
                ScanEvent::InteractionInserted => publish(SensorEvent {
                    at_ms: now_ms(),
                    source: EventSource::ModuleRight,
                    kind: EventKind::ModuleInserted,
                }),
                ScanEvent::InteractionRemoved => publish(SensorEvent {
                    at_ms: now_ms(),
                    source: EventSource::ModuleRight,
                    kind: EventKind::ModuleRemoved,
                }),
                ScanEvent::None => {}
            }
            next_scan_ms = now.saturating_add(crate::drivers::module::SCAN_PERIOD_MS);
        }

        // ---- Persistent [scan] summary every 10 s ----
        if now >= next_scan_summary_ms {
            imu::log_scan_summary();
            next_scan_summary_ms = now.saturating_add(SCAN_SUMMARY_PERIOD_MS);
        }

        // ---- 1 s stats line ----
        if now >= next_stats_ms {
            let imu_hz = imu_samples.saturating_sub(last_imu_samples);
            let bmm_hz = bmm_samples.saturating_sub(last_bmm_samples);
            last_imu_samples = imu_samples;
            last_bmm_samples = bmm_samples;

            let module_state = if !module.present() {
                "absent"
            } else if module.interact_active() {
                "interact"
            } else {
                "other"
            };
            let board_type = module.board_type().unwrap_or(0);
            let mag_count = mags.iter().filter(|device| device.is_some()).count();
            let first_mag = last_mag.iter().flatten().next().copied();
            let mag_ut = first_mag.map(|sample| sample.magnitude_ut).unwrap_or(0.0);
            let mag_raw = first_mag.map(|sample| sample.raw).unwrap_or([0; 3]);

            info!(
                "sensor: imu={imu_hz}Hz bmm={bmm_hz}Hz q={} drops={} edrops={} mod={module_state}(0x{board_type:02X}) mags={mag_count} mag={mag_ut:.1}uT raw=[{},{},{}] ierr={imu_errors} iskip={imu_skips} berr={bmm_errors} bskip={bmm_empty}",
                SENSOR_EVENTS.len(),
                SENSOR_DROPS.load(Ordering::Relaxed),
                SENSOR_EDGE_DROPS.load(Ordering::Relaxed),
                mag_raw[0],
                mag_raw[1],
                mag_raw[2],
            );
            // IMU timing is a separate line so the main stats line stays
            // below the 158-byte console record limit.
            info!(
                "sensor timing: imu_read={imu_read_us}us imax={imu_read_max_us}us late={late_us}us lmax={late_max_us}us miss={missed_ticks}"
            );

            // Battery is on its own line: the gauge poll is 5 s and carries
            // voltage/current/temperature, which do not belong on the 1 s
            // sensor line.
            match last_battery {
                Some(sample) => info!(
                    "sensor battery: soc={}% mv={} avg_ma={} c={} status=0x{:04X} err={battery_errors}",
                    sample.percent,
                    sample.voltage_mv,
                    sample.average_current_ma,
                    sample.temperature_c(),
                    sample.status,
                ),
                None => {
                    let state = if battery.is_some() { "waiting" } else { "absent" };
                    info!("sensor battery: {state} err={battery_errors}");
                }
            }

            // One sample line per online magnetometer (address, bus, vector and
            // classifier baseline). `SensorEvent` has no magnetic-sample
            // variant, so this log is the raw sample view; edges go to the
            // event queue above.
            for (index, sample) in last_mag.iter().enumerate() {
                let (Some(sample), Some(device)) = (sample, mags[index].as_ref()) else {
                    continue;
                };
                info!(
                    "sensor mag[{index}] {}@0x{:02X}: x={:.1} y={:.1} z={:.1} |b|={:.1}uT raw=[{},{},{}] base={:.1}uT",
                    device.bus().name(),
                    device.address(),
                    sample.x_ut,
                    sample.y_ut,
                    sample.z_ut,
                    sample.magnitude_ut,
                    sample.raw[0],
                    sample.raw[1],
                    sample.raw[2],
                    classifiers[index].baseline_ut(),
                );
            }

            next_stats_ms = now.saturating_add(STATS_PERIOD_MS);
        }
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
fn publish(event: SensorEvent) {
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
            | EventKind::ModuleInserted
            | EventKind::ModuleRemoved
    )
}

fn now_ms() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis()
}

fn now_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}
