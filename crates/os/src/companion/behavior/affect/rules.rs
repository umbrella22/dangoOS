//! Existing thresholds, durations and priority levels for the affect engine.

// ---------------------------------------------------------------------------
// Thresholds and durations
// ---------------------------------------------------------------------------

pub(super) const ACCEL_REST_MG: i32 = 80;
pub(super) const ACCEL_SMALL_MG: i32 = 300;
/// A burst with a sample above this is no small "nudge" for the anger count.
pub(super) const ACCEL_LARGE_MG: i32 = 1_200;
pub(super) const GYRO_REST_DPS: i32 = 20;
/// A rotation this fast is shaky (feeds the dizzy bucket); alone it never panics.
pub(super) const GYRO_SHAKY_DPS: i32 = 150;
/// The ±2 g range saturates near here on a hard shake along any axis.
pub(super) const ACCEL_VIOLENT_MG: i32 = 950;
/// A violent peak re-arms once the dynamic accel falls below this.
pub(super) const ACCEL_VIOLENT_RELEASE_MG: i32 = 600;
pub(crate) const VIOLENT_PEAKS: usize = 5;
pub(crate) const VIOLENT_WINDOW_MS: u64 = 1_000;
/// Dizzy bucket: gain caps, leak and trigger level.
pub(super) const DIZZY_ACCEL_CAP_MG: i32 = 1_000;
pub(super) const DIZZY_GYRO_CAP_DPS: i32 = 500;
pub(super) const DIZZY_LEAK_PER_S: u32 = 500;
pub(crate) const DIZZY_TRIGGER: u32 = 1_500;
pub(super) const DIZZY_MAX: u32 = 3_000;
pub(super) const REST_CONFIRM_MS: u64 = 300;
pub(super) const BURST_GAP_MIN_MS: u64 = 300;
pub(super) const BURST_GAP_MAX_MS: u64 = 800;
pub(super) const BURST_WINDOW_MS: u64 = 15_000;
pub(super) const ANGRY_BURST_COUNT: u8 = 5;
pub(super) const PANIC_HOLD_MS: u64 = 1_500;
pub(super) const PANIC_STILL_MS: u64 = 5_000;
pub(super) const PANIC_CONFUSED_MS: u64 = 3_000;
pub(crate) const DIZZY_MS: u64 = 2_500;
pub(super) const ANGRY_HOLD_MS: u64 = 10_000;
pub(super) const ANGRY_COOLDOWN_MS: u64 = 30_000;

pub(super) const TOUCH_PET_HOLD_MS: u64 = 1_500;
pub(super) const TOUCH_POKE_MS: u64 = 1_500;
pub(super) const RAPID_POKE_WINDOW_MS: u64 = 1_500;
pub(super) const RAPID_POKE_COUNT: u8 = 3;

pub(crate) const KEY_HAPPY_MS: u64 = 3_000;
pub(crate) const KEY_PUZZLED_MS: u64 = 3_000;
pub(super) const KEY_TRIPLE_WINDOW_MS: u64 = 1_500;
pub(super) const KEY_TRIPLE_COUNT: u8 = 3;
pub(super) const HUG_WINDOW_MS: u64 = 300;
pub(crate) const HUG_MS: u64 = 3_500;

pub(super) const LIGHT_HISTORY: usize = 16;
pub(crate) const LIGHT_FAST_WINDOW_MS: u64 = 400;
/// A fast change must be at least this many points...
pub(super) const LIGHT_FAST_MIN: i32 = 8;
/// ...and this share (percent) of the brighter of the two levels.
pub(super) const LIGHT_FAST_PCT: i32 = 40;
pub(crate) const LIGHT_BRIGHT_MS: u64 = 3_000;
/// Peering lasts this long after the last covered sample...
pub(crate) const LIGHT_DARK_MS: u64 = 3_000;
/// ...but at most this long in one cover: then the eyes have adjusted.
pub(crate) const LIGHT_COVER_MAX_MS: u64 = 15_000;
pub(super) const LIGHT_DARK_LEVEL: u8 = 10;
pub(super) const LIGHT_WAKE_LEVEL: u8 = 20;
pub(super) const NIGHT_SLEEP_MS: u64 = 60_000;
pub(super) const DAWN_WINDOW_MS: u64 = 30_000;
pub(super) const DAWN_RISE: u8 = 20;
pub(crate) const DAWN_CURIOUS_MS: u64 = 3_000;

pub(super) const PIR_DEBOUNCE_MS: u64 = 200;
pub(super) const PIR_SUSTAINED_MS: u64 = 10_000;
pub(crate) const PIR_SATISFIED_MS: u64 = 3_000;
pub(super) const PIR_SATISFIED_COOLDOWN_MS: u64 = 30_000;
pub(super) const PIR_ABSENCE_SLEEP_MS: u64 = 60_000;
pub(crate) const PIR_CURIOUS_MS: u64 = 4_000;
/// A PIR low shorter than this is the same visit. The module pulses high for
/// a couple of seconds and re-triggers while someone moves nearby, so without
/// this every pulse would greet again.
pub(crate) const PIR_VISIT_GAP_MS: u64 = 30_000;

pub(super) const BMM_APPROACH_MS: u64 = 2_000;
pub(super) const BMM_CONTACT_MS: u64 = 1_500;
pub(super) const BMM_RELEASE_MS: u64 = 2_000;
pub(super) const BMM_ORBIT_MS: u64 = 1_200;
pub(super) const BMM_EDGE_MS: u64 = 1_500;
pub(super) const BMM_CONFIDENCE_MIN: u8 = 50;

pub(crate) const INTERACTION_INSERT_SURPRISED_MS: u64 = 800;
pub(crate) const INTERACTION_INSERT_CURIOUS_MS: u64 = 2_000;
pub(crate) const INTERACTION_INSERT_HAPPY_MS: u64 = 2_000;
pub(crate) const INTERACTION_REMOVE_SURPRISED_MS: u64 = 800;
pub(crate) const INTERACTION_REMOVE_CONFUSED_MS: u64 = 2_500;
pub(crate) const CAMERA_INSERT_CURIOUS_MS: u64 = 2_500;
pub(crate) const CAMERA_REMOVE_CONFUSED_MS: u64 = 2_500;

pub(super) const HAPTIC_RATE_LIMIT_MS: u64 = 150;

/// A reaction stays on screen at least this long before a passive sensor
/// (light, PIR, magnet) from a different source may replace it at the same
/// priority. Deliberate input (keys, touch, shaking) and follow-ups from the
/// same source still switch at once.
pub(crate) const MIN_DWELL_MS: u64 = 1_200;

/// No sleep timer fires within this long of the last interaction.
pub(crate) const AWAKE_HOLD_MS: u64 = 5 * 60_000;
/// Drowsy face after a gentle wake.
pub(crate) const WAKE_DROWSY_MS: u64 = 3_000;
/// Grumpy face after a rough wake.
pub(crate) const WAKE_GRUMPY_MS: u64 = 2_500;

/// How the interaction that may wake her felt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rouse {
    /// A stroke, a key, lifting her: drowsy.
    Gentle,
    /// A poke or a tap: drowsy, and grumpy if repeated while drowsy.
    Poke,
    /// A knock: grumpy.
    Rough,
}

/// Tire below this state of charge.
pub(super) const BATTERY_TIRED_PCT: u8 = 15;
/// Leave `Tired` at or above this level (hysteresis).
pub(super) const BATTERY_TIRED_CLEAR_PCT: u8 = 18;
/// Half-shut eyes below this state of charge.
pub(super) const BATTERY_DYING_PCT: u8 = 5;
/// Leave `Dying` at or above this level (hysteresis).
pub(super) const BATTERY_DYING_CLEAR_PCT: u8 = 8;

pub(super) const PRIO_AMBIENT: u8 = 1;
pub(super) const PRIO_PHYSICAL: u8 = 2;
pub(super) const PRIO_ANGRY: u8 = 3;
pub(super) const PRIO_PANIC: u8 = 4;
