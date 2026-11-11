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
fn slow_roll_scoots_through_the_full_circle_without_a_false_slip_at_the_seam() {
    let mut b = Balance::new(0);
    let mut saw_scoot = false;
    for at in (20..=6200).step_by(20) {
        let angle = (at as f32 - 200.0).max(0.0) * 0.06;
        let v = sample(&mut b, at, angle, 60);
        saw_scoot |= v.stage == BalanceStage::Scooting;
        assert!(
            matches!(v.stage, BalanceStage::Resting | BalanceStage::Scooting),
            "{at}: {v:?}"
        );
        if angle < 89.5 {
            assert_eq!(v.angle_deg, 0.0, "{at}: {v:?}");
            assert_eq!(v.stage, BalanceStage::Resting);
        } else if angle > 270.0 {
            assert!(wrap(v.angle_deg - angle).abs() < 15.0, "{at}: {v:?}");
        }
    }
    assert!(saw_scoot);
}

#[test]
fn sustained_fast_roll_has_a_visible_grip_attempt_then_a_fall_and_soft_landing() {
    let mut b = Balance::new(0);
    for at in (20..=200).step_by(20) {
        sample(&mut b, at, 0.0, 0);
    }
    let mut stages = std::vec::Vec::new();
    for at in (220..=3200).step_by(20) {
        let angle = ((at - 200) as f32 * 0.36).min(288.0);
        let v = sample(&mut b, at, angle, if at <= 1000 { 360 } else { 0 });
        if stages.last() != Some(&v.stage) {
            stages.push(v.stage);
        }
        assert!(v.x.abs() <= 38.1 && v.y.abs() <= 38.1);
        assert!(v.scale > 0.5 && v.scale <= 1.01);
    }
    let grip = stages
        .iter()
        .position(|s| *s == BalanceStage::Struggling)
        .unwrap();
    let fall = stages
        .iter()
        .position(|s| *s == BalanceStage::Falling)
        .unwrap();
    let landing = stages
        .iter()
        .position(|s| *s == BalanceStage::Landing)
        .unwrap();
    assert!(grip < fall && fall < landing, "{stages:?}");
    assert!(
        matches!(
            stages.last(),
            Some(BalanceStage::Resting | BalanceStage::Scooting)
        ),
        "{stages:?}"
    );
}

#[test]
fn stopping_during_the_grip_window_saves_the_companion() {
    let mut b = Balance::new(0);
    for at in (20..=200).step_by(20) {
        sample(&mut b, at, 0.0, 0);
    }
    let mut grip = false;
    let mut rescued = false;
    let mut previous = BalanceVisual::REST;
    for at in (220..=2200).step_by(20) {
        let angle = ((at - 200) as f32 * 0.36).min(165.6);
        let v = sample(&mut b, at, angle, if at < 660 { 360 } else { 0 });
        grip |= v.stage == BalanceStage::Struggling;
        rescued |= v.stage == BalanceStage::Recovering;
        if v.stage == BalanceStage::Recovering && previous.stage == BalanceStage::Struggling {
            assert!(
                (v.grip - previous.grip).abs() < 0.1,
                "rescue must retain the grip before releasing it"
            );
            assert!(
                (v.drag - previous.drag).abs() < 0.1,
                "rescue must retain the body extension"
            );
        }
        previous = v;
        assert_ne!(v.stage, BalanceStage::Falling, "{at}: {v:?}");
    }
    assert!(grip && rescued);
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

#[test]
fn a_faster_roll_has_a_heavier_landing_squash() {
    let peak = |speed: f32| {
        let mut b = Balance::new(0);
        let mut peak = 0.0f32;
        for at in (20u64..=3500).step_by(20) {
            let moving = at.saturating_sub(200).min(900);
            let v = sample(
                &mut b,
                at,
                moving as f32 * speed / 1000.0,
                if at > 200 && moving < 900 {
                    speed as i16
                } else {
                    0
                },
            );
            if v.stage == BalanceStage::Landing {
                peak = peak.max(v.squash);
            }
        }
        peak
    };
    let normal = peak(300.0);
    let heavy = peak(600.0);
    assert!(
        normal > 0.15 && heavy > normal + 0.07,
        "normal={normal}, heavy={heavy}"
    );
}

#[test]
fn rotation_faces_progress_to_dizzy_then_weary_only_after_stopping() {
    for turn_ms in [900, 3000] {
        let mut balance = Balance::new(0);
        let mut faces = std::vec::Vec::new();
        for at in (20u64..=turn_ms + 3500).step_by(20) {
            let moving = at.saturating_sub(500).min(turn_ms);
            let turning = at > 500 && moving < turn_ms;
            let visual = sample(
                &mut balance,
                at,
                moving as f32 * 0.6,
                if turning { 600 } else { 0 },
            );
            if faces.last() != Some(&visual.face) {
                faces.push(visual.face);
            }
            if turning {
                assert_ne!(visual.face, BalanceFace::Weary, "still rotating at {at}");
            }
        }
        assert_eq!(
            faces,
            [
                BalanceFace::Neutral,
                BalanceFace::Effort,
                BalanceFace::Dizzy,
                BalanceFace::Weary,
                BalanceFace::Neutral,
            ],
            "turn duration {turn_ms}"
        );
    }
}

fn assert_held(actual: BalanceVisual, held: BalanceVisual) {
    assert_eq!(actual.stage, BalanceStage::Resting, "{actual:?}");
    assert_eq!(actual.angle_deg, held.angle_deg, "body angle changed");
    assert_eq!((actual.x, actual.y), (held.x, held.y), "body moved");
    assert_eq!(actual.scale, held.scale, "body resized");
    assert_eq!(
        (actual.reach, actual.step, actual.grip, actual.drag),
        (0.0, 0.0, 0.0, 0.0)
    );
}

#[test]
fn pickup_and_fast_sub_quarter_turns_keep_the_body_and_paws_still() {
    for speed in [5.0f32, 60.0, 360.0, 900.0] {
        for direction in [-1.0f32, 1.0] {
            let mut b = Balance::new(0);
            let duration = (60_000.0 / speed) as u64;
            for at in (20..=duration + 1200).step_by(20) {
                let moving = at.min(duration);
                let angle = direction * (moving as f32 * speed / 1000.0).min(60.0);
                let v = sample(
                    &mut b,
                    at,
                    angle,
                    if at < duration { speed as i16 } else { 0 },
                );
                assert_held(v, BalanceVisual::REST);
                assert_eq!(v.face, BalanceFace::Neutral);
            }
        }
    }
    let mut b = Balance::new(0);
    for at in (20..=5000).step_by(20) {
        let v = sample(&mut b, at, sin(at as f32 / 120.0) * 88.0, 1000);
        assert_held(v, BalanceVisual::REST);
    }
}

#[test]
fn slow_and_fast_turns_start_at_or_beyond_ninety_and_reach_the_new_direction() {
    for speed in [5.0f32, 30.0, 90.0, 360.0, 900.0] {
        for end in [90.0f32, 120.0, 180.0, 270.0, 450.0] {
            for direction in [-1.0f32, 1.0] {
                let mut b = Balance::new(0);
                let duration = (end * 1000.0 / speed) as u64;
                let mut started_at = None;
                let mut last = BalanceVisual::REST;
                for at in (20..=duration + 4500).step_by(20) {
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
                    if angle.abs() < 89.5 {
                        assert_held(v, BalanceVisual::REST);
                    }
                    if started_at.is_none() && v.stage != BalanceStage::Resting {
                        assert!(
                            angle.abs() >= 89.5,
                            "early start: {speed}/{end}/{direction}: {v:?}"
                        );
                        started_at = Some(at);
                    }
                    if speed <= 60.0
                        && moving < duration
                        && started_at.is_some_and(|started| at > started + 2500)
                    {
                        assert!(
                            wrap(v.angle_deg - angle).abs() < 4.0,
                            "continuous slow turn paused: {speed}/{end}/{direction} at {at}: {v:?}"
                        );
                    }
                    last = v;
                }
                assert!(started_at.is_some(), "missed {speed}/{end}/{direction}");
                assert!(
                    wrap(last.angle_deg - direction * end).abs() < 2.0,
                    "did not settle at {speed}/{end}/{direction}: {last:?}"
                );
                assert_eq!(last.stage, BalanceStage::Resting);
            }
        }
    }
}

#[test]
fn an_unconfirmed_crossing_cancels_and_a_delayed_tick_cannot_confirm_one_sample() {
    let mut b = Balance::new(0);
    for at in (20..=200).step_by(20) {
        sample(&mut b, at, 0.0, 0);
    }
    sample(&mut b, 220, 92.0, 900);
    assert_held(b.tick(480), BalanceVisual::REST);
    for at in (500..=3000).step_by(20) {
        // Brief excursions past 90 degrees return below the cancellation band.
        let angle = if at % 400 < 80 { 92.0 } else { 79.0 };
        assert_held(sample(&mut b, at, angle, 900), BalanceVisual::REST);
    }
    for at in (3020..=3080).step_by(20) {
        assert_held(sample(&mut b, at, 91.0, 0), BalanceVisual::REST);
    }
    for at in (3100..=4000).step_by(20) {
        assert_held(sample(&mut b, at, 89.0, 0), BalanceVisual::REST);
    }
    assert_ne!(sample(&mut b, 4020, 91.0, 0).stage, BalanceStage::Resting);
}

#[test]
fn a_settled_turn_holds_through_jitter_and_rearms_relative_to_its_new_direction() {
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
    assert!((held.angle_deg - 90.0).abs() < 1.5);
    for at in (4020..=7000).step_by(20) {
        let angle = 90.0 + sin(at as f32 / 100.0) * 12.0;
        assert_held(sample(&mut b, at, angle, 500), held);
    }
    for at in (7020..=9000).step_by(20) {
        assert_held(sample(&mut b, at, 150.0, 0), held);
    }
    // Returning to upright is a fresh 90-degree turn from the settled sideways pose.
    let mut moved = false;
    for at in (9020..=12000).step_by(20) {
        let angle = (150.0 - (at - 9000) as f32 * 0.15).max(0.0);
        let v = sample(&mut b, at, angle, if angle > 0.0 { 150 } else { 0 });
        if angle > 0.5 {
            assert_held(v, held);
        }
        moved |= v.stage != BalanceStage::Resting;
        if at == 12000 {
            assert!(v.angle_deg.abs() < 1.5, "{v:?}");
        }
    }
    assert!(moved);
}
