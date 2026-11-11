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
        assert!(wrap(v.angle_deg - angle).abs() < 15.0, "{at}: {v:?}");
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
    for at in (220..=1600).step_by(20) {
        let angle = ((at - 200) as f32 * 0.36).min(86.4);
        let v = sample(&mut b, at, angle, if at <= 440 { 360 } else { 0 });
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
