use fan_core::fan::{automatic_duty, estimated_power};

#[test]
fn automatic_curve_matches_the_shared_control_contract() {
    for (temperature, duty) in [
        (-20.0, 25),
        (29.9, 25),
        (30.0, 25),
        (35.0, 37),
        (40.0, 50),
        (45.0, 62),
        (50.0, 75),
        (55.0, 87),
        (60.0, 100),
        (100.0, 100),
    ] {
        assert_eq!(automatic_duty(temperature), duty, "{temperature}°C");
    }
    assert_eq!(automatic_duty(f32::NAN), 100);
}

#[test]
fn displayed_power_uses_the_same_rating_on_both_platforms() {
    assert_eq!(estimated_power(0), 0.0);
    assert_eq!(estimated_power(25), 2.0);
    assert_eq!(estimated_power(75), 6.0);
    assert_eq!(estimated_power(100), 8.0);
}
