use super::*;
use crate::companion::behavior::affect::EventSource;

fn sample(balance: &mut Balance, at: u64, angle: f32, speed: i16) -> BalanceVisual {
    for kind in [
        EventKind::Accel {
            ax_mg: (sin(angle / DEG) * 1000.0) as i16,
            ay_mg: (cos(angle / DEG) * 1000.0) as i16,
            az_mg: 0,
        },
        EventKind::Gyro {
            gx_dps: 0,
            gy_dps: 0,
            gz_dps: speed,
        },
    ] {
        balance.observe(SensorEvent {
            at_ms: at,
            source: EventSource::Imu,
            kind,
        });
    }
    balance.tick(at)
}

#[test]
fn a_flat_device_and_missing_or_invalid_gravity_never_invent_a_fall() {
    let mut b = Balance::new(0);
    for at in (20..=400).step_by(20) {
        b.observe(SensorEvent {
            at_ms: at,
            source: EventSource::Imu,
            kind: EventKind::Accel {
                ax_mg: 0,
                ay_mg: 0,
                az_mg: 1000,
            },
        });
        assert_eq!(b.tick(at), BalanceVisual::REST);
    }
    sample(&mut b, 420, 60.0, 0);
    assert!(b.tick(440).active);
    assert_eq!(b.tick(800), BalanceVisual::REST);
    b.observe(SensorEvent {
        at_ms: 820,
        source: EventSource::Imu,
        kind: EventKind::Accel {
            ax_mg: 0,
            ay_mg: 0,
            az_mg: 0,
        },
    });
    assert_eq!(b.tick(840), BalanceVisual::REST);
}

#[test]
fn old_samples_do_not_reverse_the_latest_gravity() {
    let mut b = Balance::new(0);
    for at in (20..=400).step_by(20) {
        sample(&mut b, at, 90.0, 0);
    }
    let target = b.target;
    sample(&mut b, 100, -90.0, 500);
    assert_eq!(b.target, target);
    assert_eq!(b.gyro_speed, 0.0);
}

fn assert_held(actual: BalanceVisual, held: BalanceVisual) {
    assert_eq!(actual.stage, BalanceStage::Resting, "{actual:?}");
    assert_eq!(
        (actual.angle_deg, actual.x, actual.y, actual.scale),
        (held.angle_deg, held.x, held.y, held.scale)
    );
    assert_eq!(
        (actual.reach, actual.step, actual.grip, actual.drag),
        (0.0, 0.0, 0.0, 0.0)
    );
}

#[test]
fn pickup_and_small_fast_turns_keep_the_body_and_paws_still() {
    for speed in [5.0f32, 60.0, 360.0, 900.0] {
        for direction in [-1.0f32, 1.0] {
            let mut b = Balance::new(0);
            let duration = (55_000.0 / speed) as u64;
            for at in (20..=duration + 1200).step_by(20) {
                let angle = direction * (at.min(duration) as f32 * speed / 1000.0).min(55.0);
                assert_held(
                    sample(
                        &mut b,
                        at,
                        angle,
                        if at < duration { speed as i16 } else { 0 },
                    ),
                    BalanceVisual::REST,
                );
            }
        }
    }
    let mut b = Balance::new(0);
    for at in (20..=5000).step_by(20) {
        assert_held(
            sample(&mut b, at, sin(at as f32 / 120.0) * 55.0, 1000),
            BalanceVisual::REST,
        );
    }
}

#[test]
fn imprecise_turns_choose_a_side_at_every_speed_without_resizing() {
    for speed in [5.0f32, 30.0, 90.0, 360.0, 900.0] {
        for (end, expected) in [
            (70.0f32, 90.0f32),
            (80.0, 90.0),
            (95.0, 90.0),
            (110.0, 90.0),
            (160.0, 180.0),
            (175.0, 180.0),
            (190.0, 180.0),
            (210.0, 180.0),
            (250.0, 270.0),
            (280.0, 270.0),
            (345.0, 360.0),
            (450.0, 450.0),
        ] {
            for direction in [-1.0f32, 1.0] {
                let mut b = Balance::new(0);
                let duration = (end * 1000.0 / speed) as u64;
                let mut moved = false;
                let mut last = BalanceVisual::REST;
                for at in (20..=duration + 6000).step_by(20) {
                    let moving = at.saturating_sub(200).min(duration);
                    let angle = direction * (moving as f32 * speed / 1000.0).min(end);
                    let v = sample(
                        &mut b,
                        at,
                        angle,
                        if at > 200 && moving < duration {
                            speed as i16
                        } else {
                            0
                        },
                    );
                    moved |= v.stage != BalanceStage::Resting;
                    assert_eq!(v.scale, 1.0, "uniform shrink at {speed}/{end}/{direction}");
                    assert!(v.x.abs() <= 38.1 && v.y.abs() <= 38.1);
                    last = v;
                }
                assert!(moved, "missed {speed}/{end}/{direction}");
                assert_eq!(
                    last.stage,
                    BalanceStage::Resting,
                    "{speed}/{end}/{direction}: {last:?}"
                );
                assert!(
                    wrap(last.angle_deg - direction * expected).abs() < 0.1,
                    "wrong side at {speed}/{end}/{direction}: {last:?}"
                );
            }
        }
    }
}

#[test]
fn sensor_jitter_cannot_retarget_a_crawl_or_keep_it_moving_after_arrival() {
    let mut clean = Balance::new(0);
    let mut noisy = Balance::new(0);
    let mut saw_crawl = false;
    let mut arrival = None;
    for at in (20..=8000).step_by(20) {
        let base = ((at as f32 - 200.0).max(0.0) * 0.09).min(90.0);
        let jitter = if at > 1200 {
            sin(at as f32 / 95.0) * 12.0
        } else {
            0.0
        };
        let a = sample(&mut clean, at, base, if at < 1200 { 90 } else { 0 });
        let b = sample(
            &mut noisy,
            at,
            base + jitter,
            if at < 1200 { 90 } else { 200 },
        );
        assert_eq!(
            (a.stage, a.angle_deg, a.x, a.y, a.scale, a.reach, a.step),
            (b.stage, b.angle_deg, b.x, b.y, b.scale, b.reach, b.step),
            "jitter changed the action at {at}"
        );
        saw_crawl |= a.stage == BalanceStage::Scooting;
        if saw_crawl && a.stage == BalanceStage::Resting {
            arrival.get_or_insert(at);
        }
        if let Some(arrived) = arrival {
            assert!(at < 4000 || arrived < 3000);
            assert_held(b, a);
        }
    }
    assert!(saw_crawl && arrival.is_some());
}

#[test]
fn a_quarter_turn_crawls_for_a_visible_interval_and_a_fast_one_does_not_fall() {
    for speed in [30.0f32, 360.0, 900.0] {
        let mut b = Balance::new(0);
        let duration = (90_000.0 / speed) as u64;
        let mut first = None;
        let mut last = None;
        let mut paw_peak = 0.0f32;
        for at in (20..=duration + 4000).step_by(20) {
            let moving = at.saturating_sub(200).min(duration);
            let v = sample(
                &mut b,
                at,
                moving as f32 * speed / 1000.0,
                if at > 200 && moving < duration {
                    speed as i16
                } else {
                    0
                },
            );
            assert!(matches!(
                v.stage,
                BalanceStage::Resting | BalanceStage::Scooting
            ));
            if v.stage == BalanceStage::Scooting {
                first.get_or_insert(at);
                last = Some(at);
                paw_peak = paw_peak.max(v.step.abs());
            }
        }
        assert!(
            last.unwrap() - first.unwrap() >= 1200,
            "crawl was a short snap at {speed}"
        );
        assert!(paw_peak > 0.9);
    }
}

#[test]
fn stopping_after_a_fast_half_turn_still_panics_falls_squashes_then_looks_weary() {
    for speed in [300.0f32, 600.0, 900.0] {
        for end in [155.0f32, 175.0, 180.0, 195.0, 210.0] {
            for direction in [-1.0f32, 1.0] {
                let mut b = Balance::new(0);
                let duration = (end * 1000.0 / speed) as u64;
                let mut stages = std::vec::Vec::new();
                let mut faces = std::vec::Vec::new();
                let mut peak = 0.0f32;
                for at in (20..=duration + 4000).step_by(20) {
                    let moving = at.saturating_sub(200).min(duration);
                    let v = sample(
                        &mut b,
                        at,
                        direction * (moving as f32 * speed / 1000.0).min(end),
                        if at > 200 && moving < duration {
                            speed as i16
                        } else {
                            0
                        },
                    );
                    if stages.last() != Some(&v.stage) {
                        stages.push(v.stage);
                    }
                    if faces.last() != Some(&v.face) {
                        faces.push(v.face);
                    }
                    if v.stage == BalanceStage::Landing {
                        peak = peak.max(v.squash);
                    }
                    if v.face == BalanceFace::Weary {
                        assert!(!matches!(
                            v.stage,
                            BalanceStage::Struggling | BalanceStage::Falling
                        ));
                    }
                }
                let phases = [
                    BalanceStage::Struggling,
                    BalanceStage::Falling,
                    BalanceStage::Landing,
                    BalanceStage::Recovering,
                ];
                let positions: std::vec::Vec<_> = phases
                    .iter()
                    .map(|stage| stages.iter().position(|s| s == stage).unwrap())
                    .collect();
                assert!(
                    positions.windows(2).all(|pair| pair[0] < pair[1]),
                    "{stages:?}"
                );
                assert!(
                    peak > 0.27,
                    "no heavy squash: {speed}/{end}/{direction}: {peak}"
                );
                assert_eq!(
                    faces,
                    [
                        BalanceFace::Neutral,
                        BalanceFace::Startled,
                        BalanceFace::Weary,
                        BalanceFace::Neutral
                    ],
                    "{speed}/{end}/{direction}"
                );
            }
        }
    }
}

#[test]
fn slower_half_turns_crawl_and_continuous_slow_rolls_do_not_slip_at_the_seam() {
    for (speed, end) in [(30.0f32, 180.0f32), (60.0, 450.0)] {
        let mut b = Balance::new(0);
        let duration = (end * 1000.0 / speed) as u64;
        for at in (20..=duration + 4000).step_by(20) {
            let moving = at.saturating_sub(200).min(duration);
            let v = sample(
                &mut b,
                at,
                moving as f32 * speed / 1000.0,
                if at > 200 && moving < duration {
                    speed as i16
                } else {
                    0
                },
            );
            assert!(
                matches!(v.stage, BalanceStage::Resting | BalanceStage::Scooting),
                "{at}: {v:?}"
            );
        }
    }
}

#[test]
fn an_unconfirmed_crossing_cancels_and_ui_delay_cannot_confirm_one_sample() {
    let mut b = Balance::new(0);
    for at in (20..=200).step_by(20) {
        sample(&mut b, at, 0.0, 0);
    }
    sample(&mut b, 220, 68.0, 900);
    assert_held(b.tick(480), BalanceVisual::REST);
    for at in (500..=3000).step_by(20) {
        let angle = if at % 400 < 80 { 68.0 } else { 49.0 };
        assert_held(sample(&mut b, at, angle, 900), BalanceVisual::REST);
    }
    for at in (3020..=3060).step_by(20) {
        assert_held(sample(&mut b, at, 66.0, 0), BalanceVisual::REST);
    }
    for at in (3080..=4000).step_by(20) {
        assert_held(sample(&mut b, at, 64.0, 0), BalanceVisual::REST);
    }
    assert_ne!(sample(&mut b, 4020, 66.0, 0).stage, BalanceStage::Resting);
}

#[test]
fn the_new_side_holds_through_jitter_and_a_reverse_turn_crawls_back() {
    let mut b = Balance::new(0);
    let mut held = BalanceVisual::REST;
    for at in (20..=4000).step_by(20) {
        let angle = ((at as f32 - 200.0).max(0.0) * 0.09).min(90.0);
        held = sample(
            &mut b,
            at,
            angle,
            if (220..1200).contains(&at) { 90 } else { 0 },
        );
    }
    assert!((held.angle_deg - 90.0).abs() < 0.1);
    for at in (4020..=7000).step_by(20) {
        assert_held(
            sample(&mut b, at, 90.0 + sin(at as f32 / 100.0) * 12.0, 500),
            held,
        );
    }
    for at in (7020..=9000).step_by(20) {
        assert_held(sample(&mut b, at, 150.0, 0), held);
    }
    let mut moved = false;
    for at in (9020..=13000).step_by(20) {
        let angle = (150.0 - (at - 9000) as f32 * 0.15).max(0.0);
        let v = sample(&mut b, at, angle, if angle > 0.0 { 150 } else { 0 });
        if angle > 26.0 {
            assert_held(v, held);
        }
        moved |= v.stage != BalanceStage::Resting;
        if at == 13000 {
            assert!(v.angle_deg.abs() < 0.1);
        }
    }
    assert!(moved);
}
