//! What the character feels and shows: sensors, buttons and touch in, one
//! [`Expression`] and one haptic pattern out.
//!
//! Pure `core` Rust with caller-supplied millisecond timestamps: no hardware,
//! no LVGL, no wall clock. The same code runs in host tests and on the device.
//!
//! - [`affect`]: sensor and button events (shake, magnet, light, presence,
//!   modules, battery) become an expression and a haptic pattern.
//! - [`personality`]: the tap ladder (curious, puzzled, annoyed, angry) with
//!   its noticing delay, hold times and anger cooldown.
//! - [`runtime`]: merges both into the expression the screen shows, with a
//!   fixed priority so a touch reaction is not overwritten by ambient noise.

pub mod affect;
pub mod magnet;
pub mod personality;
pub mod runtime;

pub use runtime::Runtime;

/// The 18 expressions the behavior engines can ask for.
///
/// `Tired` and `Dying` are battery baselines: they replace the resting face,
/// not a reaction. The fox has two more poses of its own (see
/// [`crate::fox::FoxPose`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expression {
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
    /// Low battery.
    Tired,
    /// Critical battery.
    Dying,
    /// Sudden dark: wide eyes, dilated pupils, straining to see.
    Peering,
    /// Sudden bright light: eyes squeezed shut, a paw shading them.
    Dazzled,
}

impl Expression {
    pub const ALL: [Self; 18] = [
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
        Self::Peering,
        Self::Dazzled,
    ];
}

#[cfg(test)]
mod tests {
    mod affect_events;
    mod ambient_timeline;
    mod keys_buttons;
    mod magnetic_events;
    mod module_events;
    mod personality;
    mod shake_timeline;
    mod touch_gestures;
}
