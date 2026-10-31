//! Event, reaction and output contracts shared by behavior and device producers.
use crate::companion::behavior::Expression;

// ---------------------------------------------------------------------------
// Public event contract
// ---------------------------------------------------------------------------

/// The producer of a [`SensorEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSource {
    Imu,
    Touch,
    Light,
    Pir,
    KeyLeft,
    KeyRight,
    ButtonPower,
    ButtonBoot,
    Bmm150,
    ModuleRight,
    ModuleLeft,
    /// BQ27220 fuel gauge (state of charge).
    Battery,
    /// Voice/LLM/other system layer input; carries [`EventKind::Activity`].
    System,
}

/// The slot modules the engine reacts to. The reaction follows the module,
/// not the slot: an Interaction board plays the same sequence on either side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    /// Official Interaction board (`board_type 0x16`).
    Interaction,
    /// Official camera (`board_type 0x07`, left slot only).
    Camera,
}

/// The payload of one sensor event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// Accelerometer sample in milli-g.
    Accel {
        ax_mg: i16,
        ay_mg: i16,
        az_mg: i16,
    },
    /// Gyroscope sample in degrees per second.
    Gyro {
        gx_dps: i16,
        gy_dps: i16,
        gz_dps: i16,
    },
    /// A petting gesture is in progress.
    TouchPet,
    /// A short poke with little movement.
    TouchPoke,
    /// Filtered relative light level, `0..=100` (not lux).
    LightLevel(u8),
    /// Sampled PIR state. Edges are debounced by the engine.
    Presence(bool),
    KeyDown,
    KeyUp,
    /// `button_power` short press (PTT).
    ButtonShort,
    /// `button_power` long press (hub/drawer).
    ButtonLong,
    /// `button_power` double press (stop/cancel).
    ButtonDouble,
    MagneticApproach,
    /// Edge-triggered: repeated contacts without a release are ignored.
    MagneticContact,
    /// Edge-triggered: ignored unless a contact is active.
    MagneticRelease,
    MagneticOrbit,
    /// Edge change with classifier confidence `0..=100`.
    MagneticEdge {
        confidence: u8,
    },
    /// A module was inserted in the slot named by [`SensorEvent::source`].
    ModuleInserted(ModuleKind),
    /// A module was removed from the slot named by [`SensorEvent::source`].
    ModuleRemoved(ModuleKind),
    /// BQ27220 state of charge, `0..=100`.
    BatteryLevel(u8),
    /// External voice/LLM Activity input. Accepts `Listening`, `Thinking`,
    /// `Speaking`, `Error` and `Idle`; other expressions are ignored.
    Activity(Expression),
    /// Actual link edges, selected until a subsequent host state replaces them.
    NetworkConnection(bool),
    BluetoothConnection(bool),
    /// Active playback owns MusicPlaying; false releases it immediately.
    MusicPlayback(bool),
}

/// One timestamped sensor event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorEvent {
    pub at_ms: u64,
    pub source: EventSource,
    pub kind: EventKind,
}

/// Why the current expression was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactionReason {
    Boot,
    Idle,
    Activity,
    ShakeLarge,
    ShakeSmall,
    ShakeBursts,
    TouchPet,
    TouchPoke,
    Light,
    Presence,
    KeyLeft,
    KeyRight,
    BothKeys,
    ButtonPower,
    Magnetic,
    ModuleInsert,
    ModuleRemove,
    /// The resting face changed because of the battery.
    Battery,
    /// Woken from sleep by an interaction (drowsy or grumpy face).
    Wake,
    Error,
}

impl ReactionReason {
    /// Sensors that react to the surroundings rather than to the user's hand.
    pub(super) fn is_passive(self) -> bool {
        matches!(
            self,
            ReactionReason::Light | ReactionReason::Presence | ReactionReason::Magnetic
        )
    }
}

/// The resolved expression plus its provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffectState {
    pub expression: Expression,
    pub intensity: u8,
    pub since_ms: u64,
    pub reason: ReactionReason,
}

/// A motor pattern from RFC 0004 section 6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapticPattern {
    Panic,
    Angry,
    Surprised,
    Puzzled,
    Happy,
    Satisfied,
    Confused,
    Listening,
    Thinking,
    Error,
    ModuleInsert,
    ModuleRemove,
    MagneticContact,
}

impl HapticPattern {
    /// RFC 0004 section 6: Error > Panic > everything else.
    pub const fn priority(self) -> u8 {
        match self {
            HapticPattern::Error => 3,
            HapticPattern::Panic => 2,
            _ => 1,
        }
    }
}

/// A one-shot system action for the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemCommand {
    Hub,
    StopActivity,
    StartListening,
}

/// The result of [`super::AffectEngine::tick`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffectTick {
    pub state: AffectState,
    pub changed: bool,
    pub haptic: Option<HapticPattern>,
    pub command: Option<SystemCommand>,
}
