"""Run the production LDR lifecycle against the S31 configured-channel contract.

The HAL model rejects reads from channels absent at Adc::new and records pin
mode changes. It does not simulate ADC voltages or hardware timing; on-device
light samples and camera/slot checks remain separate evidence.
"""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SOURCE = ROOT / "crates/os/src/platform/drivers/subboard/interaction.rs"


def ldr_source(path=SOURCE):
    source = path.read_text()
    return source[source.index("struct Ldr<P, A>"):source.index("/// Capacitive keys")]


HARNESS = r'''
use std::{cell::Cell, marker::PhantomData, rc::Rc};
#[path = "__SIGNAL__"] mod signal;
use signal::LdrFilter;
macro_rules! info { ($($arg:tt)*) => { let _ = format_args!($($arg)*); }; }
trait AdcChannel { fn adc_channel(&self) -> u8; }
trait AnalogPin { fn set_analog(&self); }
trait RegisterAccess {
    fn created(&self);
    fn sample(&self, channel: u8) -> u16;
}
struct Blocking;
#[derive(Debug)] enum Error { ChannelNotConfigured(u8) }
enum Attenuation { _11dB }
struct AdcPin<P, A> { pin: P, _unit: PhantomData<A> }
struct AdcConfig<A> { configured: [bool; 8], _unit: PhantomData<A> }
impl<A> AdcConfig<A> {
    fn new() -> Self { Self { configured: [false; 8], _unit: PhantomData } }
    fn enable_pin<P: AdcChannel + AnalogPin>(&mut self, pin: P, _: Attenuation) -> AdcPin<P, A> {
        pin.set_analog();
        self.configured[pin.adc_channel() as usize] = true;
        AdcPin { pin, _unit: PhantomData }
    }
}
struct Adc<'a, A, M> { unit: A, configured: [bool; 8], _lifetime: PhantomData<(&'a (), M)> }
impl<A: RegisterAccess> Adc<'_, A, Blocking> {
    fn new(unit: A, config: AdcConfig<A>) -> Result<Self, Error> {
        unit.created();
        Ok(Self { unit, configured: config.configured, _lifetime: PhantomData })
    }
    fn read_blocking<P: AdcChannel>(&mut self, pin: &mut AdcPin<P, A>) -> Result<u16, Error> {
        let channel = pin.pin.adc_channel();
        if !self.configured[channel as usize] { return Err(Error::ChannelNotConfigured(channel)); }
        Ok(self.unit.sample(channel))
    }
}
#[derive(Default)] struct State { analog: Cell<bool>, created: Cell<u32>, conversions: Cell<u32> }
struct Pin { channel: u8, state: Rc<State> }
impl AdcChannel for Pin { fn adc_channel(&self) -> u8 { self.channel } }
impl AnalogPin for Pin { fn set_analog(&self) { self.state.analog.set(true); } }
struct Unit(Rc<State>);
impl RegisterAccess for Unit {
    fn created(&self) { self.0.created.set(self.0.created.get() + 1); }
    fn sample(&self, channel: u8) -> u16 {
        self.0.conversions.set(self.0.conversions.get() + 1);
        if channel % 2 == 0 { 1200 } else { 3200 }
    }
}
fn port(channel: u8) -> (Ldr<Pin, Unit>, Rc<State>) {
    let state = Rc::new(State::default());
    (Ldr::new(Pin { channel, state: state.clone() }, Unit(state.clone())), state)
}
#[test]
fn idle_keeps_the_module_pin_and_adc_untouched() {
    let (mut ldr, state) = port(3);
    assert!(!state.analog.get(), "idle camera RESET pin switched to analog");
    assert_eq!(state.created.get(), 0, "ADC initialized before module identification");
    assert_eq!(ldr.read(), None);
    assert_eq!(state.conversions.get(), 0);
}
#[test]
fn registers_both_slot_channels_before_the_first_read() {
    for channel in [3, 4] {
        let (mut ldr, state) = port(channel);
        ldr.activate();
        assert!(ldr.read().is_some(), "active channel {channel} was not configured at Adc::new");
        assert_eq!(state.created.get(), 1);
        assert_eq!(state.conversions.get(), 1);
        assert!(state.analog.get());
        ldr.deactivate();
        assert_eq!(ldr.read(), None);
        assert!(ldr.idle.is_some());
        ldr.activate();
        assert!(ldr.read().is_some(), "reinserted channel {channel} lost its registration");
        assert_eq!(state.created.get(), 1, "ADC peripheral initialized twice");
    }
}
#[test]
fn camera_can_borrow_and_return_left_reset_before_or_after_interaction() {
    for previously_active in [false, true] {
        let (mut ldr, state) = port(3);
        if previously_active { ldr.activate(); assert!(ldr.read().is_some()); }
        // The production InteractionPort camera lease takes this idle pin,
        // returns it after capture, and lets the next activation restore analog.
        ldr.deactivate();
        let reset = ldr.idle.take().expect("left RESET not returned");
        reset.state.analog.set(false);
        assert_eq!(ldr.read(), None);
        assert!(!state.analog.get());
        ldr.idle = Some(reset);
        ldr.deactivate();
        ldr.activate();
        assert!(state.analog.get());
        assert!(ldr.read().is_some(), "camera lease lost ADC registration");
        assert_eq!(state.created.get(), 1);
    }
}
'''


def run_lifecycle(source, directory):
    script, binary = directory / "ldr.rs", directory / "ldr"
    harness = HARNESS.replace("__SIGNAL__", str(ROOT / "crates/os/src/platform/drivers/subboard/signal.rs"))
    script.write_text(harness + "\n" + source)
    build = subprocess.run(["rustc", "--edition=2021", "--test", str(script), "-o", str(binary)], capture_output=True, text=True)
    if build.returncode:
        raise AssertionError(build.stdout + build.stderr)
    return subprocess.run([str(binary), "--test-threads=1"], capture_output=True, text=True, timeout=5)


class InteractionAdcTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("rustc"), "Rust compiler required")
    def test_lifecycle_and_unregistered_channel_regression(self):
        source = ldr_source()
        with tempfile.TemporaryDirectory(prefix="mosaico-interaction-adc-") as tmp:
            directory = pathlib.Path(tmp)
            result = run_lifecycle(source, directory)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            mutation = source.replace("Adc::new(adc, config)", "Adc::new(adc, AdcConfig::new())")
            self.assertNotEqual(mutation, source, "channel-registration mutation did not apply")
            result = run_lifecycle(mutation, directory)
            self.assertNotEqual(result.returncode, 0, "unregistered ADC channel was accepted")
            self.assertIn("was not configured at Adc::new", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
