use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use super::{
    devices::MockTempSensor,
    state::{CpuTempSensor, FanControlState, FanController, TemperatureSensor},
};

struct TemperatureReadings(Mutex<VecDeque<Result<f32, &'static str>>>);

impl TemperatureSensor for TemperatureReadings {
    fn read_celsius(&self) -> Result<f32, &'static str> {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .expect("test must supply a temperature reading for each update")
    }
}

#[derive(Default)]
struct FanOutput {
    duty: u8,
    reject_next: bool,
}

struct TestFan(Arc<Mutex<FanOutput>>);

impl FanController for TestFan {
    fn set_duty_cycle(&mut self, percentage: u8) -> Result<(), &'static str> {
        let mut output = self.0.lock().unwrap();
        if std::mem::take(&mut output.reject_next) {
            return Err("fan command rejected");
        }
        output.duty = percentage;
        Ok(())
    }

    fn read_rpm(&self) -> Result<u16, &'static str> {
        Ok(0)
    }
}

struct TestCpu;

impl CpuTempSensor for TestCpu {
    fn read_celsius(&self) -> Result<i32, &'static str> {
        Ok(42)
    }
}

fn state_with_readings(
    readings: impl IntoIterator<Item = Result<f32, &'static str>>,
) -> (FanControlState, Arc<Mutex<FanOutput>>) {
    let output = Arc::new(Mutex::new(FanOutput::default()));
    let state = FanControlState::new_hardware(
        Box::new(TemperatureReadings(Mutex::new(
            readings.into_iter().collect(),
        ))),
        Box::new(TestFan(Arc::clone(&output))),
        Box::new(TestCpu),
    );
    (state, output)
}

fn assert_duty(state: &FanControlState, output: &Mutex<FanOutput>, expected: u8) {
    assert_eq!(state.fan_duty, expected, "reported duty");
    assert_eq!(output.lock().unwrap().duty, expected, "device output");
}

#[test]
fn mock_temperature_preserves_default_hot_and_negative_baselines() {
    for base in [28.5, 60.0, -10.0] {
        let sensor = MockTempSensor::new(base);
        let reading = sensor.read_celsius().unwrap();
        assert!(
            (base - 0.5..=base + 0.5).contains(&reading),
            "base {base}°C produced {reading}°C"
        );
    }
}

#[test]
fn auto_tracks_temperature_in_both_directions() {
    let (mut state, output) = state_with_readings([Ok(25.0), Ok(60.0), Ok(40.0), Ok(25.0)]);
    state.update().unwrap();
    state.set_fan_speed(-1).unwrap();
    assert_duty(&state, &output, 25);

    for expected in [100, 50, 25] {
        state.update().unwrap();
        assert_duty(&state, &output, expected);
    }
}

#[test]
fn manual_selection_disables_auto_and_auto_can_be_reselected() {
    let (mut state, output) = state_with_readings([Ok(25.0), Ok(60.0), Ok(40.0)]);
    state.update().unwrap();
    state.set_fan_speed(-1).unwrap();
    state.set_fan_speed(75).unwrap();

    state.update().unwrap();
    assert_duty(&state, &output, 75);

    state.set_fan_speed(-1).unwrap();
    assert_duty(&state, &output, 100);
    state.update().unwrap();
    assert_duty(&state, &output, 50);
}

#[test]
fn auto_keeps_last_output_when_temperature_read_fails() {
    let (mut state, output) =
        state_with_readings([Ok(60.0), Err("temperature unavailable"), Ok(25.0)]);
    state.update().unwrap();
    state.set_fan_speed(-1).unwrap();
    assert_duty(&state, &output, 100);

    state.update().unwrap();
    assert_duty(&state, &output, 100);

    state.update().unwrap();
    assert_duty(&state, &output, 25);
}

#[test]
fn failed_auto_command_is_retried_without_reporting_unapplied_duty() {
    let (mut state, output) = state_with_readings([Ok(25.0), Ok(60.0), Ok(60.0)]);
    state.update().unwrap();
    state.set_fan_speed(-1).unwrap();
    output.lock().unwrap().reject_next = true;

    assert_eq!(state.update(), Err("fan command rejected"));
    assert_duty(&state, &output, 25);

    state.update().unwrap();
    assert_duty(&state, &output, 100);
}
