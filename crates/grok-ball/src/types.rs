//! Shared types frozen for the data, engine, rasterizer and affect modules.
//!
//! The 259-unit source coordinate space and the 48-point eye rings come from
//! the pinned MIT-licensed `tycoding/grok-ball` data. The fields mirror the
//! subset ported in RFC 0003; decorative animations and sequences are a later
//! work package.

/// Number of points in one eye ring.
pub const RING_POINTS: usize = 48;

/// Number of eye-ring sets in the upstream expression table.
pub const RING_SETS: usize = 25;

/// Number of expression seeds in the upstream catalog.
pub const SEED_COUNT: usize = 32;

/// Number of points in the blob body outline.
pub const BODY_POINTS: usize = 96;

/// A point in the source coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
}

/// The left and right closed eye outlines for one expression ring set.
#[derive(Clone, Copy, Debug)]
pub struct EyeRings {
    pub left: [Point; RING_POINTS],
    pub right: [Point; RING_POINTS],
}

impl EyeRings {
    pub const EMPTY: Self = Self {
        left: [Point::ZERO; RING_POINTS],
        right: [Point::ZERO; RING_POINTS],
    };
}

/// An 8-bit RGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    /// Parse a `#RRGGBB` literal without allocating.
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xff) as u8,
            g: ((hex >> 8) & 0xff) as u8,
            b: (hex & 0xff) as u8,
        }
    }

    /// Convert to little-endian RGB565, matching the S31 canvas format.
    pub const fn to_rgb565(self) -> u16 {
        let r = (self.r as u16 >> 3) & 0x1f;
        let g = (self.g as u16 >> 2) & 0x3f;
        let b = (self.b as u16 >> 3) & 0x1f;
        (r << 11) | (g << 5) | b
    }
}

/// Body pose and color for one expression seed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyParams {
    pub y: f32,
    pub scale: f32,
    pub rotate: f32,
    pub breathe: f32,
    pub color: Rgb,
}

impl BodyParams {
    pub const DEFAULT: Self = Self {
        y: 0.0,
        scale: 1.0,
        rotate: 0.0,
        breathe: 0.0,
        color: Rgb::from_hex(0xf1eee8),
    };
}

impl Default for BodyParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One eye's pose. Values mirror the upstream seed fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyePose {
    pub open: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub x: f32,
    pub y: f32,
    pub look_x: f32,
    pub look_y: f32,
}

impl EyePose {
    pub const DEFAULT: Self = Self {
        open: 1.0,
        scale_x: 1.0,
        scale_y: 1.0,
        x: 0.0,
        y: 0.0,
        look_x: 0.0,
        look_y: 0.0,
    };
}

impl Default for EyePose {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Left and right eye poses. Upstream `eyes.both` is expanded into both sides
/// before this value is stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyePair {
    pub left: EyePose,
    pub right: EyePose,
}

impl EyePair {
    pub const DEFAULT: Self = Self {
        left: EyePose::DEFAULT,
        right: EyePose::DEFAULT,
    };
}

impl Default for EyePair {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One normalized expression descriptor.
#[derive(Clone, Copy, Debug)]
pub struct ExpressionSeed {
    pub id: u8,
    /// Eye-ring indices this expression rotates through.
    pub pool: &'static [u8],
    /// Hold time for one pool entry; zero disables rotation.
    pub pool_min_ms: u16,
    pub pool_max_ms: u16,
    pub transition_ms: u16,
    /// Zero means the expression does not blink.
    pub blink_min_ms: u16,
    pub blink_max_ms: u16,
    /// Eyelid openness multiplier; `1.0` is fully open.
    pub openness: f32,
    pub body: BodyParams,
    pub eyes: EyePair,
}

impl ExpressionSeed {
    pub const EMPTY: Self = Self {
        id: 0,
        pool: &[],
        pool_min_ms: 0,
        pool_max_ms: 0,
        transition_ms: 0,
        blink_min_ms: 0,
        blink_max_ms: 0,
        openness: 1.0,
        body: BodyParams::DEFAULT,
        eyes: EyePair::DEFAULT,
    };
}

/// The 16 expressions reachable in the sensor-enabled release.
///
/// The six `AiState` values used by voice/LLM map into this set; they are not a
/// second engine input. `Tired` and `Dying` are the battery-fed baseline
/// expressions: they replace the resting face, not a reaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BallExpression {
    Idle,
    Sleep,
    Curious,
    Happy,
    Puzzled,
    Confused,
    Surprised,
    Panic,
    Angry,
    Satisfied,
    Listening,
    Thinking,
    Speaking,
    Error,
    /// Low battery: droopy eyes, slow breathing (upstream seed 15).
    Tired,
    /// Critical battery: half-shut eyes, slumped body (upstream seed 6).
    Dying,
}

impl BallExpression {
    pub const ALL: [Self; 16] = [
        Self::Idle,
        Self::Sleep,
        Self::Curious,
        Self::Happy,
        Self::Puzzled,
        Self::Confused,
        Self::Surprised,
        Self::Panic,
        Self::Angry,
        Self::Satisfied,
        Self::Listening,
        Self::Thinking,
        Self::Speaking,
        Self::Error,
        Self::Tired,
        Self::Dying,
    ];

    /// Upstream seed id for this expression. The upstream ids are decimal
    /// strings (`"02"`, `"35"`), not hexadecimal literals.
    pub const fn seed_id(self) -> u8 {
        match self {
            Self::Sleep => 0,
            Self::Idle => 2,
            Self::Curious => 3,
            Self::Happy => 10,
            Self::Puzzled => 11,
            Self::Surprised => 13,
            Self::Panic => 17,
            Self::Satisfied => 19,
            Self::Confused => 20,
            Self::Angry => 21,
            Self::Thinking => 30,
            Self::Error => 34,
            Self::Listening => 35,
            Self::Speaking => 39,
            Self::Tired => 15,
            Self::Dying => 6,
        }
    }

    pub const fn from_seed_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Sleep),
            2 => Some(Self::Idle),
            3 => Some(Self::Curious),
            6 => Some(Self::Dying),
            10 => Some(Self::Happy),
            11 => Some(Self::Puzzled),
            13 => Some(Self::Surprised),
            15 => Some(Self::Tired),
            17 => Some(Self::Panic),
            19 => Some(Self::Satisfied),
            20 => Some(Self::Confused),
            21 => Some(Self::Angry),
            30 => Some(Self::Thinking),
            34 => Some(Self::Error),
            35 => Some(Self::Listening),
            39 => Some(Self::Speaking),
            _ => None,
        }
    }
}

/// The six voice/LLM states. These map into [`BallExpression`] and do not
/// introduce a second expression enum in the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Happy,
    Error,
}

impl AiState {
    pub const fn expression(self) -> BallExpression {
        match self {
            Self::Idle => BallExpression::Idle,
            Self::Listening => BallExpression::Listening,
            Self::Thinking => BallExpression::Thinking,
            Self::Speaking => BallExpression::Speaking,
            Self::Happy => BallExpression::Happy,
            Self::Error => BallExpression::Error,
        }
    }
}
