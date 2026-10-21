//! Right-slot Interaction module watcher and input reader.
//!
//! ## Hardware facts (official BSP)
//!
//! - v1.2 routes the subboard bus to the **separate I2C1** controller:
//!   SDA `GPIO0`, SCL `GPIO1` (`bsp/esp_mosaico.h`,
//!   `components/esp-mosaico-bsp/onboard/subboard.c`). It is not on the main
//!   I2C0 with the touch controller and IMU.
//! - The right slot EEPROM is `0x51` when the address-select pin
//!   `GPIO39` is driven high; `main` holds it high before the task starts.
//! - The V1 descriptor is `0x86` bytes, magic `"ESP"`, `board_type` at offset
//!   `0x03`, three CRC16 sections at `0x34`, `0x3E` and `0x84`
//!   (`components/mosaico_module_mgr/mosaico_module_mgr.c`).
//! - The Interaction board type is `0x16`; the official manager scans every
//!   250 ms and requires three consecutive matching probes.
//! - Right-slot Interaction pins (left-slot canonical -> right-slot actual):
//!   LDR `GPIO53 -> GPIO46` (ADC1 channel 4), PIR `GPIO4 -> GPIO5`,
//!   KEY_L `GPIO13 -> GPIO11`, KEY_R `GPIO12 -> GPIO10`, WS2812
//!   `GPIO15 -> GPIO38` (`subboard.c` `s_gpio_pairs`). Only the four input
//!   signals are used here.
//! - LDR conversion follows `mosaico_module_interact.c`: ADC1 channel 4 is an
//!   even channel (N-side), so the raw code decreases as light rises. The
//!   module computes `2196 - raw`, clamps it to `0..=2196`, and filters it
//!   with a 3:1 IIR before scaling to `0..=100`.
//!
//! ## Event policy
//!
//! A module is *present* when its EEPROM acknowledges. Three consecutive
//! scans (750 ms worst case, 500 ms typical) confirm a change. The
//! `ModuleInserted` / `ModuleRemoved` `SensorEvent` is published only when the
//! descriptor is valid and `board_type == 0x16`; a wrong type or invalid
//! descriptor is logged and counted but produces no expression, matching RFC
//! 0004 section 4.7. Inputs (LDR/PIR/keys) are only sampled while that
//! Interaction descriptor is active.

use esp_hal::{
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation},
    gpio::{Input, InputConfig, InputPin, Pull},
    peripherals::{ADC1, GPIO46},
    Blocking,
};
use log::{info, warn};

use crate::drivers::shared_i2c;

/// Right-slot EEPROM 7-bit address (GPIO39 high).
pub const RIGHT_EEPROM_ADDR: u8 = 0x51;
/// `MOSAICO_BOARD_TYPE_INTERACT`.
pub const BOARD_TYPE_INTERACT: u8 = 0x16;
/// Official module manager cadence.
pub const SCAN_PERIOD_MS: u64 = 250;
/// Official module manager debounce count.
pub const DEBOUNCE_SAMPLES: u8 = 3;

const EEPROM_MAGIC: [u8; 3] = *b"ESP";
const DESCRIPTOR_LEN: usize = 0x86;
const DESC_CRC_OFFSET: usize = 0x34;
const MFG_DATA_OFFSET: usize = 0x36;
const MFG_CRC_OFFSET: usize = 0x3E;
const PARAM_DATA_OFFSET: usize = 0x40;
const PARAM_CRC_OFFSET: usize = 0x84;
const PARAM_LENGTH_OFFSET: usize = 0x42;
const PARAM_CAPACITY: usize = 64;

// BSP `mosaico_module_interact.c` S31 LDR constant. Channel 4 is an N-side
// input, so the usable span is `0..=LDR_ZERO_CODE` (the BSP's `4393` high
// code applies to odd, P-side channels).
const LDR_ZERO_CODE: i32 = 2196;

/// Right-slot input pins plus the LDR ADC handle.
pub struct ModulePins {
    adc: Adc<'static, ADC1<'static>, Blocking>,
    ldr: AdcPin<GPIO46<'static>, ADC1<'static>>,
    pir: Input<'static>,
    key_l: Input<'static>,
    key_r: Input<'static>,
}

impl ModulePins {
    /// Configure the four right-slot inputs. LDR uses ADC1 channel 4 with the
    /// single S31 attenuation setting; PIR is active-high with a pull-down;
    /// both keys are active-low with pull-ups (official interact component).
    pub fn new(
        ldr: GPIO46<'static>,
        adc: ADC1<'static>,
        pir: impl InputPin + 'static,
        key_l: impl InputPin + 'static,
        key_r: impl InputPin + 'static,
    ) -> Self {
        let mut adc_config = AdcConfig::new();
        let ldr = adc_config.enable_pin(ldr, Attenuation::_11dB);
        let adc = Adc::new(adc, adc_config);

        Self {
            adc,
            ldr,
            pir: Input::new(pir, InputConfig::default().with_pull(Pull::Down)),
            key_l: Input::new(key_l, InputConfig::default().with_pull(Pull::Up)),
            key_r: Input::new(key_r, InputConfig::default().with_pull(Pull::Up)),
        }
    }
}

/// One poll of the Interaction inputs. `*_edge` is `Some(level)` only after a
/// three-sample debounced transition.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModuleInputs {
    pub light_raw: u16,
    pub light_level: u8,
    pub pir: bool,
    pub key_l: bool,
    pub key_r: bool,
    pub pir_edge: Option<bool>,
    pub key_l_edge: Option<bool>,
    pub key_r_edge: Option<bool>,
}

/// The stable presence/descriptor change produced by one scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanEvent {
    None,
    InteractionInserted,
    InteractionRemoved,
}

/// Right-slot module watcher.
pub struct ModuleWatcher {
    pins: ModulePins,
    present: bool,
    candidate_present: bool,
    candidate_count: u8,
    board_type: Option<u8>,
    descriptor_valid: bool,
    interact: bool,
    announced: bool,
    key_l: Debounce,
    key_r: Debounce,
    pir: Debounce,
    ldr_filtered_q8: i32,
    ldr_valid: bool,
}

impl ModuleWatcher {
    pub fn new(pins: ModulePins) -> Self {
        Self {
            pins,
            present: false,
            candidate_present: false,
            candidate_count: 0,
            board_type: None,
            descriptor_valid: false,
            interact: false,
            announced: false,
            key_l: Debounce::new(),
            key_r: Debounce::new(),
            pir: Debounce::new(),
            ldr_filtered_q8: 0,
            ldr_valid: false,
        }
    }

    pub fn present(&self) -> bool {
        self.present
    }

    pub fn board_type(&self) -> Option<u8> {
        self.board_type
    }

    pub fn interact_active(&self) -> bool {
        self.interact
    }

    /// Probe the EEPROM once. Call every [`SCAN_PERIOD_MS`].
    pub async fn scan(&mut self) -> ScanEvent {
        let present = probe_eeprom().await;

        if present == self.candidate_present {
            self.candidate_count = self.candidate_count.saturating_add(1).min(DEBOUNCE_SAMPLES);
        } else {
            self.candidate_present = present;
            self.candidate_count = 1;
        }

        if self.candidate_count < DEBOUNCE_SAMPLES || present == self.present {
            return ScanEvent::None;
        }

        self.present = present;
        if present {
            match read_descriptor().await {
                Ok((board_type, valid)) => {
                    self.board_type = Some(board_type);
                    self.descriptor_valid = valid;
                    self.interact = valid && board_type == BOARD_TYPE_INTERACT;
                    if self.interact {
                        info!(
                            "right-slot module inserted: type=0x{board_type:02X} ({}) descriptor=valid",
                            board_type_name(board_type)
                        );
                        self.announced = true;
                        ScanEvent::InteractionInserted
                    } else if valid {
                        warn!(
                            "right-slot module inserted: type=0x{board_type:02X} ({}) is not Interaction; inputs disabled",
                            board_type_name(board_type)
                        );
                        self.announced = false;
                        ScanEvent::None
                    } else {
                        warn!("right-slot module inserted: EEPROM descriptor invalid; inputs disabled");
                        self.announced = false;
                        ScanEvent::None
                    }
                }
                Err(error) => {
                    self.board_type = None;
                    self.descriptor_valid = false;
                    self.interact = false;
                    self.announced = false;
                    warn!("right-slot module inserted: EEPROM read failed ({error})");
                    ScanEvent::None
                }
            }
        } else {
            let announced = self.announced;
            self.board_type = None;
            self.descriptor_valid = false;
            self.interact = false;
            self.announced = false;
            self.ldr_valid = false;
            self.key_l.reset();
            self.key_r.reset();
            self.pir.reset();
            if announced {
                info!("right-slot Interaction module removed");
                ScanEvent::InteractionRemoved
            } else {
                info!("right-slot module removed");
                ScanEvent::None
            }
        }
    }

    /// Sample LDR/PIR/KEY_L/KEY_R. Returns `None` while no Interaction module
    /// is active. The ADC conversion is blocking for a few microseconds; no
    /// I2C transaction is performed here.
    pub fn poll_inputs(&mut self) -> Option<ModuleInputs> {
        if !self.interact {
            return None;
        }

        let raw = self.pins.adc.read_blocking(&mut self.pins.ldr);
        // Channel 4 is even (N-side), so the code decreases with light.
        let level = (LDR_ZERO_CODE - raw as i32).clamp(0, LDR_ZERO_CODE);
        let target_q8 = (level * 100 * 256 + LDR_ZERO_CODE / 2) / LDR_ZERO_CODE;
        if !self.ldr_valid {
            self.ldr_filtered_q8 = target_q8;
            self.ldr_valid = true;
            info!(
                "Interaction LDR raw={raw} level={}% (ADC1_CH4)",
                (target_q8 + 128) / 256
            );
        } else {
            self.ldr_filtered_q8 = (self.ldr_filtered_q8 * 3 + target_q8 + 2) / 4;
        }
        let light_level = ((self.ldr_filtered_q8 + 128) / 256).clamp(0, 100) as u8;

        let key_l = self.pins.key_l.is_low();
        let key_r = self.pins.key_r.is_low();
        let pir = self.pins.pir.is_high();

        // Update the debouncers first so the snapshot fields below report the
        // post-edge stable state.
        let pir_edge = self.pir.update(pir);
        let key_l_edge = self.key_l.update(key_l);
        let key_r_edge = self.key_r.update(key_r);

        Some(ModuleInputs {
            light_raw: raw,
            light_level,
            pir: self.pir.stable,
            key_l: self.key_l.stable,
            key_r: self.key_r.stable,
            pir_edge,
            key_l_edge,
            key_r_edge,
        })
    }
}

/// Three-sample debounce matching the official module manager.
#[derive(Clone, Copy, Debug, Default)]
struct Debounce {
    stable: bool,
    candidate: bool,
    count: u8,
    initialized: bool,
}

impl Debounce {
    const fn new() -> Self {
        Self {
            stable: false,
            candidate: false,
            count: 0,
            initialized: false,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    /// Feed one raw level. Returns `Some(new_level)` once three consecutive
    /// samples disagree with the stable value.
    fn update(&mut self, raw: bool) -> Option<bool> {
        if !self.initialized {
            self.initialized = true;
            self.stable = raw;
            self.candidate = raw;
            self.count = 0;
            return None;
        }

        if raw == self.stable {
            self.candidate = raw;
            self.count = 0;
            return None;
        }

        if raw == self.candidate {
            self.count = self.count.saturating_add(1);
        } else {
            self.candidate = raw;
            self.count = 1;
        }

        if self.count >= DEBOUNCE_SAMPLES {
            self.stable = raw;
            self.count = 0;
            Some(raw)
        } else {
            None
        }
    }
}

async fn probe_eeprom() -> bool {
    let mut byte = [0u8; 1];
    matches!(
        shared_i2c::subboard_read_regs_async(RIGHT_EEPROM_ADDR, 0x00, &mut byte).await,
        Some(Ok(()))
    )
}

/// Read and validate the `0x86`-byte V1 descriptor.
async fn read_descriptor() -> Result<(u8, bool), &'static str> {
    let mut raw = [0u8; DESCRIPTOR_LEN];
    match shared_i2c::subboard_read_regs_async(RIGHT_EEPROM_ADDR, 0x00, &mut raw).await {
        Some(Ok(())) => {}
        Some(Err(_)) => return Err("I2C error"),
        None => return Err("subboard I2C not installed"),
    }

    let magic_ok = raw[..3] == EEPROM_MAGIC[..];
    let desc_crc = read_le16(&raw[DESC_CRC_OFFSET..]);
    let mfg_crc = read_le16(&raw[MFG_CRC_OFFSET..]);
    let param_crc = read_le16(&raw[PARAM_CRC_OFFSET..]);
    let param_length = read_le16(&raw[PARAM_LENGTH_OFFSET..]) as usize;
    let crcs_ok = desc_crc == crc16(&raw[..DESC_CRC_OFFSET])
        && mfg_crc == crc16(&raw[MFG_DATA_OFFSET..MFG_CRC_OFFSET])
        && param_crc == crc16(&raw[PARAM_DATA_OFFSET..PARAM_CRC_OFFSET])
        && param_length <= PARAM_CAPACITY;

    let valid = magic_ok && crcs_ok;
    if !magic_ok {
        warn!("right-slot EEPROM magic {:02X?} is not ESP", &raw[0..3]);
    } else if !crcs_ok {
        warn!("right-slot EEPROM descriptor CRC/param-length check failed");
    }

    Ok((raw[0x03], valid))
}

fn read_le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

/// CRC16-CCITT reflected, matching `mosaico_module_mgr.c`.
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn board_type_name(board_type: u8) -> &'static str {
    match board_type {
        0x01 => "Core",
        0x02 => "Power",
        0x03 => "Dock",
        0x04 => "Handle",
        0x05 => "Balance Car",
        0x06 => "Display",
        0x07 => "Camera",
        0x08 => "Sensor",
        0x09 => "IO Expansion",
        0x10 => "ToF",
        0x11 => "Matrix LED",
        0x12 => "Thermal",
        0x13 => "Relay",
        0x16 => "Interaction",
        _ => "Unknown",
    }
}
