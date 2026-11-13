//! Expansion slots: module detection and the Interaction board in either slot.
//!
//! ## Hardware facts (official BSP)
//!
//! - Both slots share the **I2C1** subboard bus (SDA `GPIO0`, SCL `GPIO1`).
//!   Each slot's EEPROM address pin is wired to a GPIO: left `GPIO14` low ->
//!   `0x50`, right `GPIO39` high -> `0x51` (`onboard/subboard.c`). The
//!   descriptor's `board_type` tells the module kind ([`descriptor`]).
//! - A module plugged into the right slot sees mirrored pins
//!   (`subboard.c` `s_gpio_pairs`, `docs/hardware/board-v1.2.md` section 7.2).
//!   For the Interaction board:
//!
//!   | Signal | Left | Right |
//!   | --- | --- | --- |
//!   | LDR | `GPIO53` (ADC2 ch 3) | `GPIO46` (ADC1 ch 4) |
//!   | PIR | `GPIO4` | `GPIO5` |
//!   | KEY_L | `GPIO13` | `GPIO11` |
//!   | KEY_R | `GPIO12` | `GPIO10` |
//!   | WS2812 | `GPIO15` | `GPIO38` |
//!
//! - The camera EEPROM is fixed at `0x50` and the camera only works in the
//!   left slot (its data lines include `GPIO14`). It is therefore reported as
//!   the left slot whatever its position; while it is present `GPIO14` is
//!   released so it does not fight the camera's D4 output.
//!
//! ## Policy
//!
//! - Every [`SCAN_PERIOD_MS`] each slot is probed once; three matching probes
//!   confirm an insert or removal, as in `mosaico_module_mgr.c`.
//! - Insert/remove changes are reported for the Interaction board and the
//!   camera. Other board types and unreadable descriptors are only logged.
//! - One Interaction board is read at a time: the first one detected. When it
//!   is removed and the other slot holds an Interaction board, that one takes
//!   over. Pins of an idle slot stay high-impedance.

pub mod descriptor;
pub mod interaction;
mod retry;
pub mod signal;
pub mod touch_key;

use esp_hal::{
    gpio::{Flex, Level, Output, OutputConfig, OutputPin, Pin},
    peripherals::{ADC1, ADC2, GPIO46, GPIO53},
};
use log::{info, warn};

use crate::companion::behavior::affect::ModuleKind;
use crate::platform::buses::shared_i2c;

use crate::platform::drivers::subboard::retry::DescriptorRetry;
use crate::platform::drivers::subboard::signal::Debounce;
use {
    crate::platform::drivers::subboard::descriptor::DescriptorError,
    crate::platform::drivers::subboard::descriptor::BOARD_TYPE_CAMERA,
    crate::platform::drivers::subboard::descriptor::BOARD_TYPE_INTERACT,
    crate::platform::drivers::subboard::descriptor::DESCRIPTOR_LEN,
};
pub use {
    crate::platform::drivers::subboard::interaction::InteractionInputs,
    crate::platform::drivers::subboard::interaction::InteractionPort,
};

/// Slot probe period (official manager: 250 ms).
pub const SCAN_PERIOD_MS: u64 = 250;

/// Left-slot Interaction pins.
pub type LeftPort = InteractionPort<GPIO53<'static>, ADC2<'static>>;
/// Right-slot Interaction pins.
pub type RightPort = InteractionPort<GPIO46<'static>, ADC1<'static>>;

/// An expansion slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Left,
    Right,
}

impl Slot {
    const ALL: [Slot; 2] = [Slot::Left, Slot::Right];

    const fn eeprom_addr(self) -> u8 {
        match self {
            Slot::Left => 0x50,
            Slot::Right => 0x51,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }

    const fn other(self) -> Slot {
        match self {
            Slot::Left => Slot::Right,
            Slot::Right => Slot::Left,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Slot::Left => "left",
            Slot::Right => "right",
        }
    }
}

/// What a slot holds after its descriptor was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Module {
    Interaction,
    Camera,
    Other(u8),
    Unreadable,
    ReadFailed,
}

impl Module {
    fn kind(self) -> Option<ModuleKind> {
        match self {
            Module::Interaction => Some(ModuleKind::Interaction),
            Module::Camera => Some(ModuleKind::Camera),
            Module::Other(_) | Module::Unreadable | Module::ReadFailed => None,
        }
    }
}

/// A confirmed insert or removal of a known module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotChange {
    pub slot: Slot,
    pub kind: ModuleKind,
    pub inserted: bool,
}

struct SlotState {
    presence: Debounce,
    module: Option<Module>,
    retry: DescriptorRetry,
}

/// Both expansion slots.
pub struct Subboard {
    slots: [SlotState; 2],
    left: LeftPort,
    right: RightPort,
    /// Slot whose Interaction board is being sampled.
    active: Option<Slot>,
    /// `GPIO14`: left EEPROM A0, also camera D4.
    left_addr: Flex<'static>,
    _right_addr: Output<'static>,
    camera_leased: bool,
}

impl Subboard {
    pub fn new(
        left_addr: impl Pin + 'static,
        right_addr: impl OutputPin + 'static,
        left: LeftPort,
        right: RightPort,
    ) -> Self {
        let mut left_addr = Flex::new(left_addr);
        drive_low(&mut left_addr);
        Self {
            slots: [Slot::Left, Slot::Right].map(|_| SlotState {
                presence: Debounce::starting_at(false),
                module: None,
                retry: DescriptorRetry::new(),
            }),
            left,
            right,
            active: None,
            left_addr,
            _right_addr: Output::new(right_addr, Level::High, OutputConfig::default()),
            camera_leased: false,
        }
    }

    /// Probe both slots once. Call every [`SCAN_PERIOD_MS`].
    pub async fn scan(&mut self) -> [Option<SlotChange>; 2] {
        let mut changes = [None; 2];
        for slot in Slot::ALL {
            let present = probe(slot).await;
            let change = match self.slots[slot.index()].presence.update(present) {
                Some(true) => self.identify_inserted(slot).await,
                Some(false) => {
                    self.slots[slot.index()].retry.reset();
                    self.slots[slot.index()].module.take().and_then(|module| {
                        self.on_removed(slot, module);
                        module.kind()
                    })
                }
                None => {
                    let state = &mut self.slots[slot.index()];
                    if state.module == Some(Module::ReadFailed) {
                        if let Some(attempt) = state.retry.scan(present) {
                            info!(
                                "{} slot: retrying EEPROM descriptor ({attempt}/{})",
                                slot.name(),
                                retry::MAX_RETRIES
                            );
                            self.identify_inserted(slot).await
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            };
            changes[slot.index()] = change.map(|kind| SlotChange {
                slot,
                kind,
                inserted: present,
            });
        }
        changes
    }

    /// Slot of the Interaction board being sampled, if any.
    pub fn active_slot(&self) -> Option<Slot> {
        self.active
    }

    pub fn service_camera(&mut self) {
        use crate::platform::drivers::camera::slot;
        slot::set_present(self.slots[0].module == Some(Module::Camera));
        if let Some(lease) = slot::returned() {
            self.left.return_camera_reset(lease.reset);
            self.camera_leased = false;
            if self.slots[0].module != Some(Module::Camera) {
                drive_low(&mut self.left_addr);
            }
            if self.active.is_none() && self.slots[0].module == Some(Module::Interaction) {
                self.activate(Slot::Left);
            }
        }
        if slot::requested() {
            let result = if self.camera_leased {
                Err("left slot leased")
            } else if self.slots[0].module != Some(Module::Camera) {
                Err("no camera in left slot")
            } else {
                self.left
                    .take_camera_reset()
                    .ok_or("left slot pin unavailable")
                    .map(|reset| {
                        release(&mut self.left_addr);
                        let [d3, d5, d7] = self.left.camera_data();
                        self.camera_leased = true;
                        slot::Lease {
                            reset,
                            data: Some([d3, self.left_addr.peripheral_input(), d5, d7]),
                        }
                    })
            };
            slot::grant(result);
        }
    }

    /// Sample the active Interaction board, if any. A few microseconds of
    /// blocking ADC conversion and touch register reads; no I2C.
    pub fn poll_inputs(&mut self) -> Option<InteractionInputs> {
        match self.active? {
            Slot::Left => self.left.sample(),
            Slot::Right => self.right.sample(),
        }
    }

    async fn identify_inserted(&mut self, slot: Slot) -> Option<ModuleKind> {
        let module = self.identify(slot).await;
        let state = &mut self.slots[slot.index()];
        state.module = Some(module);
        if module == Module::ReadFailed {
            if !state.retry.read_failed() {
                warn!(
                    "{} slot: EEPROM retries exhausted; waiting for removal",
                    slot.name()
                );
            }
        } else {
            state.retry.reset();
        }
        // The usual activation/event path is reached only after a complete
        // valid descriptor. Failed reads never configure Interaction GPIOs.
        self.on_inserted(slot, module);
        module.kind()
    }

    async fn identify(&mut self, slot: Slot) -> Module {
        let mut raw = [0u8; DESCRIPTOR_LEN];
        let read = shared_i2c::subboard_read_regs_async(slot.eeprom_addr(), 0x00, &mut raw).await;
        if !matches!(read, Some(Ok(()))) {
            warn!("{} slot: EEPROM read failed", slot.name());
            return Module::ReadFailed;
        }
        match descriptor::parse(&raw) {
            Ok(BOARD_TYPE_INTERACT) => Module::Interaction,
            Ok(BOARD_TYPE_CAMERA) => Module::Camera,
            Ok(board_type) => Module::Other(board_type),
            Err(error) => {
                match error {
                    DescriptorError::BadMagic(magic) => {
                        warn!("{} slot: EEPROM magic {magic:02X?} is not ESP", slot.name())
                    }
                    DescriptorError::BadChecksum => warn!(
                        "{} slot: descriptor CRC/length check failed (two modules on 0x{:02X}?)",
                        slot.name(),
                        slot.eeprom_addr()
                    ),
                }
                Module::Unreadable
            }
        }
    }

    fn on_inserted(&mut self, slot: Slot, module: Module) {
        match module {
            Module::Interaction if self.active.is_none() => {
                self.activate(slot);
                info!(
                    "{} slot: Interaction board inserted, inputs active",
                    slot.name()
                );
            }
            Module::Interaction => info!(
                "{} slot: Interaction board inserted, idle while the {} board is active",
                slot.name(),
                slot.other().name()
            ),
            Module::Camera => {
                release(&mut self.left_addr);
                info!("camera module detected (left slot); GPIO14 released");
            }
            Module::Other(board_type) => info!(
                "{} slot: module type 0x{board_type:02X} ({}) inserted, not handled",
                slot.name(),
                descriptor::board_type_name(board_type)
            ),
            Module::Unreadable | Module::ReadFailed => {}
        }
    }

    fn on_removed(&mut self, slot: Slot, module: Module) {
        match module {
            Module::Interaction => {
                info!("{} slot: Interaction board removed", slot.name());
                if self.active == Some(slot) {
                    self.deactivate(slot);
                    let other = slot.other();
                    if self.slots[other.index()].module == Some(Module::Interaction) {
                        self.activate(other);
                        info!("{} slot: Interaction board takes over", other.name());
                    }
                }
            }
            Module::Camera => {
                if !self.camera_leased {
                    drive_low(&mut self.left_addr);
                }
                info!("camera module removed; GPIO14 drives left A0 low again");
            }
            Module::Other(_) | Module::Unreadable | Module::ReadFailed => {
                info!("{} slot: module removed", slot.name())
            }
        }
    }

    fn activate(&mut self, slot: Slot) {
        if slot == Slot::Left && self.camera_leased {
            return;
        }
        match slot {
            Slot::Left => self.left.activate(),
            Slot::Right => self.right.activate(),
        }
        self.active = Some(slot);
    }

    fn deactivate(&mut self, slot: Slot) {
        match slot {
            Slot::Left => self.left.deactivate(),
            Slot::Right => self.right.deactivate(),
        }
        self.active = None;
    }
}

async fn probe(slot: Slot) -> bool {
    let mut byte = [0u8; 1];
    matches!(
        shared_i2c::subboard_read_regs_async(slot.eeprom_addr(), 0x00, &mut byte).await,
        Some(Ok(()))
    )
}

fn drive_low(pin: &mut Flex<'static>) {
    pin.set_input_enable(false);
    pin.apply_output_config(&OutputConfig::default());
    pin.set_low();
    pin.set_output_enable(true);
}

fn release(pin: &mut Flex<'static>) {
    pin.set_output_enable(false);
}
