//! Official Interaction board (`board_type 0x16`) on one slot.
//!
//! Each slot has its own [`InteractionPort`] that owns that side's pins. A
//! port starts **idle**: the digital pins are high-impedance with no pull and
//! the LDR pin is not yet switched to analog, so a different module on the
//! same contacts (the camera uses `GPIO4/12/13/53` on the left) is not loaded.
//! [`InteractionPort::activate`] applies the official pulls only once the
//! EEPROM says an Interaction board is plugged in.
//!
//! The keys are capacitive pads read by the S31 touch sensor
//! (`drivers::touch_sens`), as in the official BSP. If the pads read
//! implausibly the port falls back to plain GPIO keys (pull-up, active low).
//!
//! The WS2812 light is driven by `tasks::lights` (right slot only). IR TX is
//! not driven.

use esp_hal::{
    analog::adc::{Adc, AdcChannel, AdcConfig, AdcPin, Attenuation, RegisterAccess},
    gpio::{AnalogPin, Flex, InputConfig, Pin, Pull},
    Blocking,
};

use super::signal::{Debounce, LdrFilter};
use super::touch_key::{self, TouchKey};
use crate::drivers::touch_sens;
use log::{info, warn};

/// Samples between periodic touch logs (25 Hz sampling -> 60 s). Edges are
/// always logged.
const TOUCH_LOG_EVERY: u16 = 1500;

/// One sample of an active Interaction board.
#[derive(Clone, Copy, Debug)]
pub struct InteractionInputs {
    /// Filtered LDR level, `0..=100` (higher is brighter).
    pub light_level: u8,
    /// Debounced PIR edge (`true` = presence started).
    pub pir_edge: Option<bool>,
    /// Debounced KEY_L edge (`true` = pressed).
    pub key_l_edge: Option<bool>,
    /// Debounced KEY_R edge (`true` = pressed).
    pub key_r_edge: Option<bool>,
}

/// LDR on one ADC unit. The pin is switched to analog only while active.
struct Ldr<P, A> {
    adc: Option<Adc<'static, A, Blocking>>,
    peripheral: Option<A>,
    idle: Option<P>,
    active: Option<AdcPin<P, A>>,
    filter: LdrFilter,
}

impl<P, A> Ldr<P, A>
where
    P: AdcChannel + AnalogPin + 'static,
    A: RegisterAccess + 'static,
{
    fn new(pin: P, adc: A) -> Self {
        let filter = LdrFilter::new(pin.adc_channel());
        Self {
            // Defer both analog pin setup and ADC construction until a board
            // is identified. S31 ignores per-pin attenuation, but Adc::new
            // still records which channels may be read.
            adc: None,
            peripheral: Some(adc),
            idle: Some(pin),
            active: None,
            filter,
        }
    }

    fn activate(&mut self) {
        if let Some(pin) = self.idle.take() {
            let mut config = AdcConfig::<A>::new();
            let active = config.enable_pin(pin, Attenuation::_11dB);
            if let Some(adc) = self.peripheral.take() {
                self.adc = Some(Adc::new(adc, config).expect("LDR ADC configuration"));
            }
            self.active = Some(active);
            self.filter.reset();
        }
    }

    /// Return the pin. It stays in analog mode, which is high-impedance.
    fn deactivate(&mut self) {
        if let Some(active) = self.active.take() {
            self.idle = Some(active.pin);
        }
    }

    fn read(&mut self) -> Option<(u16, u8)> {
        let pin = self.active.as_mut()?;
        let raw = self.adc.as_mut()?.read_blocking(pin).ok()?;
        Some((raw, self.filter.update(raw)))
    }
}

/// Capacitive keys being read through the touch block.
struct TouchKeys {
    /// Touch channels of KEY_L and KEY_R.
    channels: [u8; 2],
    keys: [TouchKey; 2],
    /// Samples left before the benchmark is trusted.
    warmup: u8,
    log_countdown: u16,
    #[cfg(feature = "touch-key-diagnostics")]
    diagnostic_left: u16,
    // Read time, smooth values and baselines BEFORE TouchKey::update.
    #[cfg(feature = "touch-key-diagnostics")]
    diagnostic_sample: Option<(u64, [u16; 2], [u32; 2])>,
}

enum KeyMode {
    /// Port idle, keys not read.
    Off,
    Touch(TouchKeys),
    Gpio,
}

/// The Interaction board pins of one slot.
pub struct InteractionPort<P, A> {
    ldr: Ldr<P, A>,
    pir: Flex<'static>,
    key_l: Flex<'static>,
    key_r: Flex<'static>,
    /// Touch channels of KEY_L / KEY_R, when both pins have one.
    touch_channels: Option<[u8; 2]>,
    key_mode: KeyMode,
    pir_debounce: Debounce,
    key_l_debounce: Debounce,
    key_r_debounce: Debounce,
    first_sample: bool,
}

impl<P, A> InteractionPort<P, A>
where
    P: AdcChannel + AnalogPin + 'static,
    A: RegisterAccess + 'static,
{
    pub fn new(
        ldr: P,
        adc: A,
        pir: impl Pin + 'static,
        key_l: impl Pin + 'static,
        key_r: impl Pin + 'static,
    ) -> Self {
        let touch_channels = match (
            touch_sens::channel_of_gpio(key_l.number()),
            touch_sens::channel_of_gpio(key_r.number()),
        ) {
            (Some(l), Some(r)) => Some([l, r]),
            _ => None,
        };
        let mut port = Self {
            ldr: Ldr::new(ldr, adc),
            touch_channels,
            key_mode: KeyMode::Off,
            pir: Flex::new(pir),
            key_l: Flex::new(key_l),
            key_r: Flex::new(key_r),
            pir_debounce: Debounce::new(),
            key_l_debounce: Debounce::new(),
            key_r_debounce: Debounce::new(),
            first_sample: false,
        };
        port.deactivate();
        port
    }

    /// Apply the official PIR pull, start the touch keys and sampling.
    pub fn activate(&mut self) {
        Self::input_with_pull(&mut self.pir, Pull::Down);
        // The key pins stay floating with the input buffer off, which is
        // what the touch block needs.
        self.key_mode = match self.touch_channels {
            Some(channels) => {
                touch_sens::start(&channels);
                info!(
                    "touch keys: KEY_L ch{} KEY_R ch{}, calibrating",
                    channels[0], channels[1]
                );
                KeyMode::Touch(TouchKeys {
                    channels,
                    keys: [TouchKey::new(); 2],
                    warmup: touch_key::WARMUP_SAMPLES,
                    log_countdown: TOUCH_LOG_EVERY,
                    #[cfg(feature = "touch-key-diagnostics")]
                    diagnostic_left: 6000,
                    #[cfg(feature = "touch-key-diagnostics")]
                    diagnostic_sample: None,
                })
            }
            None => self.gpio_keys(),
        };
        self.ldr.activate();
        self.pir_debounce = Debounce::new();
        self.key_l_debounce = Debounce::new();
        self.key_r_debounce = Debounce::new();
        self.first_sample = true;
        #[cfg(feature = "touch-key-diagnostics")]
        if matches!(self.key_mode, KeyMode::Touch(_)) {
            info!("tkdiag armed: max6000 post-warmup samples; edge -1=none 0=up 1=down; baseline before update");
        }
    }

    fn input_with_pull(pin: &mut Flex<'static>, pull: Pull) {
        pin.set_output_enable(false);
        pin.apply_input_config(&InputConfig::default().with_pull(pull));
        pin.set_input_enable(true);
    }

    /// Plain keys: pull-up, active low.
    fn gpio_keys(&mut self) -> KeyMode {
        Self::input_with_pull(&mut self.key_l, Pull::Up);
        Self::input_with_pull(&mut self.key_r, Pull::Up);
        KeyMode::Gpio
    }

    /// Release the pins: no pull, input and output disabled.
    pub fn deactivate(&mut self) {
        if matches!(self.key_mode, KeyMode::Touch(_)) {
            touch_sens::stop();
        }
        self.key_mode = KeyMode::Off;
        for pin in [&mut self.pir, &mut self.key_l, &mut self.key_r] {
            pin.set_output_enable(false);
            pin.apply_input_config(&InputConfig::default().with_pull(Pull::None));
            pin.set_input_enable(false);
        }
        self.ldr.deactivate();
    }

    /// The slot lease suspends Interaction access until this pin is returned.
    pub fn take_camera_reset(&mut self) -> Option<P> {
        self.deactivate();
        self.ldr.idle.take()
    }

    pub fn return_camera_reset(&mut self, pin: P) {
        assert!(self.ldr.idle.is_none() && self.ldr.active.is_none());
        self.ldr.idle = Some(pin);
        self.deactivate();
    }

    pub fn camera_data(&self) -> [esp_hal::gpio::interconnect::InputSignal<'static>; 3] {
        [self.pir.peripheral_input(), self.key_r.peripheral_input(), self.key_l.peripheral_input()]
    }

    /// Sample once. `None` while idle.
    pub fn sample(&mut self) -> Option<InteractionInputs> {
        let (raw, light_level) = self.ldr.read()?;
        if core::mem::take(&mut self.first_sample) {
            log::info!("Interaction LDR raw={raw} level={light_level}%");
        }
        let (key_l, key_r) = self.read_keys();
        // TouchKey already filters noise and requires four quiet samples to
        // release. Debouncing that state again can swallow a complete release
        // between two taps. Plain GPIO fallback still needs three samples.
        let (key_l_edge, key_r_edge) = if matches!(self.key_mode, KeyMode::Touch(_)) {
            (
                self.key_l_debounce.accept_filtered(key_l),
                self.key_r_debounce.accept_filtered(key_r),
            )
        } else {
            (
                self.key_l_debounce.update(key_l),
                self.key_r_debounce.update(key_r),
            )
        };
        #[cfg(feature = "touch-key-diagnostics")]
        self.log_touch_sample(key_l_edge, key_r_edge);
        Some(InteractionInputs {
            light_level,
            pir_edge: self.pir_debounce.update(self.pir.is_high()),
            key_l_edge,
            key_r_edge,
        })
    }

    /// TouchKey-filtered or raw GPIO states (`true` = held).
    fn read_keys(&mut self) -> (bool, bool) {
        let touch = match &mut self.key_mode {
            KeyMode::Off => return (false, false),
            KeyMode::Gpio => return (self.key_l.is_low(), self.key_r.is_low()),
            KeyMode::Touch(touch) => touch,
        };
        let r = touch.channels.map(touch_sens::read);
        if touch.warmup > 0 {
            touch.warmup -= 1;
            if touch.warmup == 0 {
                if r.iter().all(|r| touch_key::plausible(r.smooth)) {
                    info!(
                        "touch keys ready: L {} press>+{} R {} press>+{}",
                        r[0].smooth,
                        touch_key::press_delta(r[0].smooth.into()),
                        r[1].smooth,
                        touch_key::press_delta(r[1].smooth.into())
                    );
                } else {
                    warn!(
                        "touch keys implausible (L {} R {}), using GPIO keys",
                        r[0].smooth, r[1].smooth
                    );
                    touch_sens::stop();
                    self.key_mode = self.gpio_keys();
                }
            }
            return (false, false);
        }
        let before = [touch.keys[0].pressed(), touch.keys[1].pressed()];
        #[cfg(feature = "touch-key-diagnostics")]
        if touch.diagnostic_left > 0 {
            touch.diagnostic_sample = Some((
                embassy_time::Instant::now().as_millis(),
                [r[0].smooth, r[1].smooth],
                [touch.keys[0].baseline(), touch.keys[1].baseline()],
            ));
        }
        let held = [
            touch.keys[0].update(r[0].smooth),
            touch.keys[1].update(r[1].smooth),
        ];
        touch.log_countdown = touch.log_countdown.saturating_sub(1);
        if held != before || touch.log_countdown == 0 {
            touch.log_countdown = TOUCH_LOG_EVERY;
            info!(
                "touch L {}/{} R {}/{} held={:?}",
                r[0].smooth,
                touch.keys[0].baseline(),
                r[1].smooth,
                touch.keys[1].baseline(),
                held
            );
        }
        (held[0], held[1])
    }

    #[cfg(feature = "touch-key-diagnostics")]
    fn log_touch_sample(&mut self, left_edge: Option<bool>, right_edge: Option<bool>) {
        let KeyMode::Touch(touch) = &mut self.key_mode else {
            return;
        };
        let Some((at, smooth, baseline)) = touch.diagnostic_sample.take() else {
            return;
        };
        touch.diagnostic_left -= 1;
        let edge = |value: Option<bool>| value.map_or(-1i8, |pressed| pressed as i8);
        // Every valid sample is retained: 1-3 quiet samples must remain visible
        // even when TouchKey has not yet emitted a release.
        info!(
            "tkdiag t={} ch={}/{} s={}/{} b={}/{} h={}/{} e={}/{}",
            at,
            touch.channels[0],
            touch.channels[1],
            smooth[0],
            smooth[1],
            baseline[0],
            baseline[1],
            touch.keys[0].pressed() as u8,
            touch.keys[1].pressed() as u8,
            edge(left_edge),
            edge(right_edge),
        );
        if touch.diagnostic_left == 0 {
            info!("tkdiag complete: 6000 post-warmup samples; reactivate port to rearm");
        }
    }
}
