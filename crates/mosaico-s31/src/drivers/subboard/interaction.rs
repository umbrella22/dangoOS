//! Official Interaction board (`board_type 0x16`) on one slot.
//!
//! Each slot has its own [`InteractionPort`] that owns that side's pins. A
//! port starts **idle**: the digital pins are high-impedance with no pull and
//! the LDR pin is not yet switched to analog, so a different module on the
//! same contacts (the camera uses `GPIO4/12/13/53` on the left) is not loaded.
//! [`InteractionPort::activate`] applies the official pulls only once the
//! EEPROM says an Interaction board is plugged in.
//!
//! The WS2812, IR TX and the SI12T touch keys are not driven yet.

use esp_hal::{
    analog::adc::{Adc, AdcChannel, AdcConfig, AdcPin, Attenuation, RegisterAccess},
    gpio::{AnalogPin, Flex, InputConfig, Pin, Pull},
    Blocking,
};

use super::signal::{Debounce, LdrFilter};

/// One sample of an active Interaction board.
#[derive(Clone, Copy, Debug)]
pub struct InteractionInputs {
    /// Filtered LDR level, `0..=100` (higher is brighter).
    pub light_level: u8,
    /// Debounced PIR edge (`true` = presence started).
    pub pir_edge: Option<bool>,
    /// Debounced KEY_L edge (`true` = pressed; active low).
    pub key_l_edge: Option<bool>,
    /// Debounced KEY_R edge (`true` = pressed; active low).
    pub key_r_edge: Option<bool>,
}

/// LDR on one ADC unit. The pin is switched to analog only while active.
struct Ldr<P, A> {
    adc: Adc<'static, A, Blocking>,
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
            // `Adc::new` ignores the per-pin part of the config on the S31;
            // pins are enabled one by one in `activate`.
            adc: Adc::new(adc, AdcConfig::new()),
            idle: Some(pin),
            active: None,
            filter,
        }
    }

    fn activate(&mut self) {
        if let Some(pin) = self.idle.take() {
            let mut config = AdcConfig::<A>::new();
            self.active = Some(config.enable_pin(pin, Attenuation::_11dB));
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
        let raw = self.adc.read_blocking(pin);
        Some((raw, self.filter.update(raw)))
    }
}

/// The Interaction board pins of one slot.
pub struct InteractionPort<P, A> {
    ldr: Ldr<P, A>,
    pir: Flex<'static>,
    key_l: Flex<'static>,
    key_r: Flex<'static>,
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
        let mut port = Self {
            ldr: Ldr::new(ldr, adc),
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

    /// Apply the official pulls (PIR down, keys up) and start sampling.
    pub fn activate(&mut self) {
        for (pin, pull) in [
            (&mut self.pir, Pull::Down),
            (&mut self.key_l, Pull::Up),
            (&mut self.key_r, Pull::Up),
        ] {
            pin.set_output_enable(false);
            pin.apply_input_config(&InputConfig::default().with_pull(pull));
            pin.set_input_enable(true);
        }
        self.ldr.activate();
        self.pir_debounce = Debounce::new();
        self.key_l_debounce = Debounce::new();
        self.key_r_debounce = Debounce::new();
        self.first_sample = true;
    }

    /// Release the pins: no pull, input and output disabled.
    pub fn deactivate(&mut self) {
        for pin in [&mut self.pir, &mut self.key_l, &mut self.key_r] {
            pin.set_output_enable(false);
            pin.apply_input_config(&InputConfig::default().with_pull(Pull::None));
            pin.set_input_enable(false);
        }
        self.ldr.deactivate();
    }

    /// Sample once. `None` while idle.
    pub fn sample(&mut self) -> Option<InteractionInputs> {
        let (raw, light_level) = self.ldr.read()?;
        if core::mem::take(&mut self.first_sample) {
            log::info!("Interaction LDR raw={raw} level={light_level}%");
        }
        Some(InteractionInputs {
            light_level,
            pir_edge: self.pir_debounce.update(self.pir.is_high()),
            key_l_edge: self.key_l_debounce.update(self.key_l.is_low()),
            key_r_edge: self.key_r_debounce.update(self.key_r.is_low()),
        })
    }
}
