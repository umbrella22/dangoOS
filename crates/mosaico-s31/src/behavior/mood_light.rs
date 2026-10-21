//! Mood light: the Interaction board's six WS2812 LEDs follow the fox.
//!
//! Pure `core` Rust with caller-supplied millisecond timestamps, like the rest
//! of [`crate::behavior`]. The firmware's light task feeds it the expression
//! on screen, the ambient light level and key presses, and sends each
//! [`Frame`] to the LEDs.
//!
//! - The strip is dark at rest (Idle, Sleep, Tired) and in the dark-room
//!   and glare poses. A reaction lights it with its colour and a smooth
//!   pattern (breathing, a chasing comet, alternating halves or a rainbow)
//!   for [`SHOW_MS`], then fades out over [`FADE_MS`] even if the
//!   expression stays. Only Angry strobes, as a joke; nothing flickers at
//!   random.
//! - A change of expression cross-fades from the last frame over
//!   [`CROSSFADE_MS`], so the strip never jumps.
//! - Brightness follows the room: full [`MAX_LEVEL`] in daylight, down to
//!   [`MIN_AMBIENT_SCALE`] of it in the dark, never off.
//! - A key press flashes the three LEDs on its side for [`SPARK_MS`].
//!
//! The LEDs are numbered 0..6 from the left key to the right key.

use super::Expression;
use crate::fox::math::{sin, TAU};

/// LEDs on the Interaction board (`MOSAICO_INTERACT_LED_COUNT`).
pub const LED_COUNT: usize = 6;
/// Per-channel ceiling at full brightness, about a tenth of the LED's range.
/// The LEDs sit right under the eyes of whoever holds the board.
pub const MAX_LEVEL: u8 = 24;
/// Brightness in a dark room, as a fraction of [`MAX_LEVEL`].
pub const MIN_AMBIENT_SCALE: f32 = 0.3;
/// A reaction lights the strip this long at full strength...
pub const SHOW_MS: u64 = 1500;
/// ...then fades out over this long.
pub const FADE_MS: u64 = 500;
/// Cross-fade between two expressions.
pub const CROSSFADE_MS: u64 = 180;
/// Key press flash.
pub const SPARK_MS: u64 = 260;
/// Surprised opens with a white flash this long.
const SURPRISE_FLASH_MS: u64 = 240;

/// One LED colour, full scale before brightness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const OFF: Self = Self::new(0, 0, 0);
    const WHITE: Self = Self::new(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn scale(self, k: f32) -> Self {
        let k = k.clamp(0.0, 1.0);
        let ch = |c: u8| (c as f32 * k + 0.5) as u8;
        Self::new(ch(self.r), ch(self.g), ch(self.b))
    }

    fn mix(a: Self, b: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t + 0.5) as u8;
        Self::new(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b))
    }

    /// Brightest of the three channels.
    pub fn max_channel(self) -> u8 {
        self.r.max(self.g).max(self.b)
    }
}

/// The six LEDs, left to right.
pub type Frame = [Rgb; LED_COUNT];

/// Which key was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy)]
enum Pattern {
    /// All LEDs rise and fall together between `floor` and 1.
    Breathe { period_ms: u32, floor: f32 },
    /// A comet runs across the strip in colour `a` over a dim `b` base.
    Chase { period_ms: u32, reverse: bool },
    /// The comet runs to one end and back.
    Wobble { period_ms: u32 },
    /// Even and odd LEDs swap between `a` and `b`.
    Alternate { period_ms: u32 },
    /// Hard on/off in `a`; off time shows `b`.
    Strobe { period_ms: u32, on_pct: u32 },
    /// A colour wheel scrolling across the strip.
    Rainbow { period_ms: u32 },
    /// LEDs off.
    Off,
}

#[derive(Clone, Copy)]
struct Style {
    a: Rgb,
    b: Rgb,
    pattern: Pattern,
    /// Extra dimming for calm moods.
    gain: f32,
}

const fn style(expression: Expression) -> Style {
    use Expression::*;
    use Pattern::*;
    const fn breathe(period_ms: u32, floor: f32) -> Pattern {
        Breathe { period_ms, floor }
    }
    let (a, b, pattern, gain) = match expression {
        // Rest, the dark room and the glare stay dark.
        Idle | Sleep | Tired | Peering | Dazzled => (Rgb::OFF, Rgb::OFF, Off, 0.0),
        Curious => (
            Rgb::new(0, 230, 255),
            Rgb::new(0, 40, 60),
            Chase {
                period_ms: 900,
                reverse: false,
            },
            1.0,
        ),
        Happy => (Rgb::OFF, Rgb::OFF, Rainbow { period_ms: 1400 }, 1.0),
        Puzzled => (
            Rgb::new(170, 60, 255),
            Rgb::new(255, 200, 40),
            Alternate { period_ms: 700 },
            0.9,
        ),
        Confused => (
            Rgb::new(160, 60, 255),
            Rgb::new(20, 60, 60),
            Wobble { period_ms: 1100 },
            0.9,
        ),
        Surprised => (Rgb::new(255, 220, 40), Rgb::OFF, breathe(500, 0.4), 1.0),
        Panic => (
            Rgb::new(255, 0, 0),
            Rgb::new(255, 255, 255),
            Chase {
                period_ms: 300,
                reverse: true,
            },
            1.0,
        ),
        // The one strobe: a cartoon tantrum.
        Angry => (
            Rgb::new(255, 10, 0),
            Rgb::new(40, 0, 0),
            Strobe {
                period_ms: 160,
                on_pct: 50,
            },
            1.0,
        ),
        Satisfied => (Rgb::new(255, 70, 140), Rgb::OFF, breathe(2000, 0.35), 0.85),
        Listening => (Rgb::new(40, 120, 255), Rgb::OFF, breathe(1600, 0.3), 0.8),
        Thinking => (
            Rgb::new(120, 80, 255),
            Rgb::new(10, 10, 50),
            Chase {
                period_ms: 1400,
                reverse: false,
            },
            0.8,
        ),
        Speaking => (Rgb::new(40, 255, 120), Rgb::OFF, breathe(700, 0.3), 0.9),
        Error => (Rgb::new(255, 0, 0), Rgb::OFF, breathe(1000, 0.0), 0.8),
        Dying => (Rgb::new(255, 0, 0), Rgb::OFF, breathe(2000, 0.0), 0.5),
    };
    Style {
        a,
        b,
        pattern,
        gain,
    }
}

/// Mood light state.
pub struct MoodLight {
    expression: Expression,
    since_ms: u64,
    /// Frame shown when the current expression started (cross-fade origin).
    from: Frame,
    /// Last frame before brightness, the next cross-fade origin.
    last: Frame,
    ambient: Option<u8>,
    spark: Option<(Side, u64)>,
}

impl Default for MoodLight {
    fn default() -> Self {
        Self::new()
    }
}

impl MoodLight {
    pub const fn new() -> Self {
        Self {
            expression: Expression::Idle,
            since_ms: 0,
            from: [Rgb::OFF; LED_COUNT],
            last: [Rgb::OFF; LED_COUNT],
            ambient: None,
            spark: None,
        }
    }

    /// The expression on screen. Restarts the pattern when it changes.
    pub fn set_expression(&mut self, expression: Expression, now_ms: u64) {
        if expression != self.expression {
            self.from = self.last;
            self.expression = expression;
            self.since_ms = now_ms;
        }
    }

    /// Relative room brightness, 0 (dark) to 100 (bright).
    pub fn set_ambient(&mut self, level: u8) {
        self.ambient = Some(level.min(100));
    }

    /// Flash the side of a pressed key.
    pub fn spark(&mut self, side: Side, now_ms: u64) {
        self.spark = Some((side, now_ms));
    }

    /// Brightness factor applied to every frame, `MIN_AMBIENT_SCALE..=1`.
    /// Without an ambient reading the strip runs at full brightness.
    pub fn ambient_scale(&self) -> f32 {
        match self.ambient {
            Some(level) => MIN_AMBIENT_SCALE + (1.0 - MIN_AMBIENT_SCALE) * level as f32 / 100.0,
            None => 1.0,
        }
    }

    /// The frame to show at `now_ms`, already scaled to [`MAX_LEVEL`].
    pub fn frame(&mut self, now_ms: u64) -> Frame {
        let t = now_ms.saturating_sub(self.since_ms);
        let style = style(self.expression);
        let mut frame = pattern_frame(self.expression, style, t);
        if t >= SHOW_MS {
            let envelope = 1.0 - (t - SHOW_MS) as f32 / FADE_MS as f32;
            frame = frame.map(|led| led.scale(envelope));
        }

        if t < CROSSFADE_MS {
            let k = t as f32 / CROSSFADE_MS as f32;
            for (led, from) in frame.iter_mut().zip(self.from) {
                *led = Rgb::mix(from, *led, k);
            }
        }

        if let Some((side, at)) = self.spark {
            let age = now_ms.saturating_sub(at);
            if age < SPARK_MS {
                let k = 0.6 * (1.0 - age as f32 / SPARK_MS as f32);
                let range = match side {
                    Side::Left => 0..LED_COUNT / 2,
                    Side::Right => LED_COUNT / 2..LED_COUNT,
                };
                for led in &mut frame[range] {
                    *led = Rgb::mix(*led, Rgb::WHITE, k);
                }
            } else {
                self.spark = None;
            }
        }

        self.last = frame;
        let k = self.ambient_scale() * MAX_LEVEL as f32 / 255.0;
        frame.map(|led| led.scale(k))
    }
}

/// Pattern output at full scale, before cross-fade and brightness.
fn pattern_frame(expression: Expression, style: Style, t: u64) -> Frame {
    let Style {
        a,
        b,
        pattern,
        gain,
    } = style;
    if expression == Expression::Surprised && t < SURPRISE_FLASH_MS {
        return [Rgb::WHITE; LED_COUNT];
    }
    let mut frame = [Rgb::OFF; LED_COUNT];
    match pattern {
        Pattern::Off => {}
        Pattern::Breathe { period_ms, floor } => {
            let wave = 0.5 - 0.5 * sin(TAU * phase(t, period_ms) + TAU * 0.25);
            let level = floor + (1.0 - floor) * wave;
            frame = [a.scale(level); LED_COUNT];
        }
        Pattern::Chase { period_ms, reverse } => {
            let mut head = phase(t, period_ms) * LED_COUNT as f32;
            if reverse {
                head = LED_COUNT as f32 - head;
            }
            comet(&mut frame, a, b, head, true);
        }
        Pattern::Wobble { period_ms } => {
            let p = phase(t, period_ms);
            let tri = if p < 0.5 { p * 2.0 } else { 2.0 - p * 2.0 };
            comet(&mut frame, a, b, tri * (LED_COUNT - 1) as f32, false);
        }
        Pattern::Alternate { period_ms } => {
            let flip = phase(t, period_ms) >= 0.5;
            for (i, led) in frame.iter_mut().enumerate() {
                *led = if (i % 2 == 0) ^ flip { a } else { b };
            }
        }
        Pattern::Strobe { period_ms, on_pct } => {
            let on = phase(t, period_ms) * 100.0 < on_pct as f32;
            frame = [if on { a } else { b }; LED_COUNT];
        }
        Pattern::Rainbow { period_ms } => {
            let p = phase(t, period_ms);
            for (i, led) in frame.iter_mut().enumerate() {
                let hue = (p + i as f32 / LED_COUNT as f32) % 1.0;
                *led = wheel(1.0 - hue);
            }
        }
    }
    frame.map(|led| led.scale(gain))
}

/// Position within the period, `0..1`.
fn phase(t: u64, period_ms: u32) -> f32 {
    let period = u64::from(period_ms.max(1));
    (t % period) as f32 / period as f32
}

/// A bright head at `head` (LED units) with a fading tail over base `b`.
/// `wrap` lets the tail continue from the far end.
fn comet(frame: &mut Frame, a: Rgb, b: Rgb, head: f32, wrap: bool) {
    let n = LED_COUNT as f32;
    for (i, led) in frame.iter_mut().enumerate() {
        let mut d = (head - i as f32).abs();
        if wrap {
            d = d.min(n - d);
        }
        let k = (1.0 - d / 2.0).max(0.0);
        *led = Rgb::mix(b, a, k * k);
    }
}

/// Fully saturated colour at `hue` in `0..1`.
fn wheel(hue: f32) -> Rgb {
    let h = (hue.clamp(0.0, 1.0) * 6.0) % 6.0;
    let x = (h % 1.0) * 255.0;
    let (up, down) = (x as u8, (255.0 - x) as u8);
    match h as u32 {
        0 => Rgb::new(255, up, 0),
        1 => Rgb::new(down, 255, 0),
        2 => Rgb::new(0, 255, up),
        3 => Rgb::new(0, down, 255),
        4 => Rgb::new(up, 0, 255),
        _ => Rgb::new(255, 0, down),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brightest(frame: &Frame) -> usize {
        (0..LED_COUNT)
            .max_by_key(|&i| frame[i].max_channel())
            .unwrap()
    }

    #[test]
    fn never_exceeds_the_ceiling() {
        for expression in Expression::ALL {
            let mut light = MoodLight::new();
            light.set_expression(expression, 0);
            for t in (0..4000).step_by(10) {
                for led in light.frame(t) {
                    assert!(led.max_channel() <= MAX_LEVEL, "{expression:?} at {t}");
                }
            }
        }
    }

    #[test]
    fn dark_room_dims_but_keeps_the_light_on() {
        let mut light = MoodLight::new();
        light.set_ambient(0);
        light.set_expression(Expression::Curious, 0);
        let peak = (0..2000)
            .step_by(10)
            .flat_map(|t| light.frame(t))
            .map(Rgb::max_channel)
            .max()
            .unwrap();
        assert!(peak > 0);
        assert!(peak as f32 <= MAX_LEVEL as f32 * MIN_AMBIENT_SCALE + 1.0);
    }

    #[test]
    fn angry_strobes() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Angry, 0);
        let on = light.frame(CROSSFADE_MS + 20)[0].r;
        let off = light.frame(CROSSFADE_MS + 100)[0].r;
        assert!(on > off * 3, "on={on} off={off}");
    }

    #[test]
    fn rest_is_dark() {
        for expression in [Expression::Idle, Expression::Sleep, Expression::Tired] {
            let mut light = MoodLight::new();
            light.set_expression(expression, 0);
            for t in (0..5000).step_by(50) {
                assert!(light.frame(t).iter().all(|led| *led == Rgb::OFF));
            }
        }
    }

    #[test]
    fn a_reaction_fades_out_even_if_it_stays() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Happy, 0);
        assert!(light.frame(SHOW_MS / 2).iter().any(|led| *led != Rgb::OFF));
        let late = light.frame(SHOW_MS + FADE_MS);
        assert!(late.iter().all(|led| *led == Rgb::OFF));
    }

    #[test]
    fn dazzled_goes_dark_after_the_fade() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Happy, 0);
        light.frame(500);
        light.set_expression(Expression::Dazzled, 1000);
        assert!(light.frame(1000).iter().any(|led| *led != Rgb::OFF));
        assert!(light
            .frame(1000 + CROSSFADE_MS)
            .iter()
            .all(|led| *led == Rgb::OFF));
    }

    #[test]
    fn a_change_starts_from_the_last_frame() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Angry, 0);
        let before = light.frame(100);
        light.set_expression(Expression::Sleep, 100);
        assert_eq!(light.frame(100), before);
    }

    #[test]
    fn curious_comet_moves() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Curious, 0);
        let start = CROSSFADE_MS;
        let a = brightest(&light.frame(start));
        let b = brightest(&light.frame(start + 300));
        assert_ne!(a, b);
    }

    #[test]
    fn surprised_opens_with_a_white_flash() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Surprised, 0);
        let flash = light.frame(CROSSFADE_MS + 10);
        assert!(flash.iter().all(|led| led.r == led.b && led.r > 0));
        let after = light.frame(SURPRISE_FLASH_MS + 10);
        assert!(after[0].r > after[0].b);
    }

    #[test]
    fn a_key_flashes_its_own_side() {
        let mut light = MoodLight::new();
        light.set_expression(Expression::Sleep, 0);
        light.frame(1000);
        light.spark(Side::Right, 1000);
        let frame = light.frame(1000);
        for i in 0..LED_COUNT / 2 {
            assert!(frame[i + LED_COUNT / 2].b >= frame[i].b);
            assert!(frame[i + LED_COUNT / 2].g > frame[i].g);
        }
        let later = light.frame(1000 + SPARK_MS);
        assert_eq!(later[5].g, later[0].g);
    }
}
