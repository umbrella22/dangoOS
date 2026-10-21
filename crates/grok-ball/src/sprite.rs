//! Sprite animation playback for replaceable character art.
//!
//! The procedural engine draws every expression from the upstream vector data,
//! which is why the ball needs no art at all. Character art (a struggle
//! animation, a tail, a fully custom creature) arrives as bitmaps instead; this
//! module is the runtime side of that pipeline.
//!
//! # Data contract
//!
//! A [`Sprite`] is a `&'static` table produced by
//! `crates/grok-ball/tools/sprite_pack.py`:
//!
//! ```text
//! pub const SIZE: usize = 240;
//! pub const FRAME_MS: u32 = 80;
//! pub const LOOPING: bool = true;
//! pub const FRAME_COUNT: usize = 12;
//! pub static FRAMES: [&[u16; SIZE * SIZE]; FRAME_COUNT] = ...;
//! ```
//!
//! The table is RGB565 little-endian, already composited over black, which is
//! exactly what the canvas holds.
//!
//! # Resolution
//!
//! Sprites are stored at whatever canvas size the artist produced (the tool
//! defaults to 240x240). [`blit_scaled`] nearest-neighbour scales them to the
//! widget canvas at draw time, so one asset set works for the 240x240 device
//! and the 320x320 host preview. Nearest-neighbour is deliberate: pixel art
//! must not be smeared.
//!
//! # Selection
//!
//! [`SpriteBank`] maps an [`Animation`] to an optional sprite. A bank with no
//! sprites is the current firmware state: the procedural renderer draws
//! everything and none of this code runs.

use crate::affect::HapticPattern;
use crate::personality::PersonaReaction;
use crate::types::BallExpression;

/// One static sprite table.
pub struct Sprite {
    pub size: usize,
    pub frame_ms: u32,
    pub looping: bool,
    pub frames: &'static [&'static [u16]],
}

impl Sprite {
    /// Number of frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Frame index at `elapsed_ms`, or `None` when a one-shot has finished.
    pub fn frame_at(&self, elapsed_ms: u64) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let step = self.frame_ms.max(1) as u64;
        let index = (elapsed_ms / step) as usize;
        if self.looping {
            Some(index % self.frames.len())
        } else if index < self.frames.len() {
            Some(index)
        } else {
            // A one-shot holds its last frame; `None` would make the caller
            // fall back to the procedural face and lose the final pose.
            Some(self.frames.len() - 1)
        }
    }

    /// Whether a one-shot animation has reached its last frame.
    pub fn finished(&self, elapsed_ms: u64) -> bool {
        if self.looping || self.frames.is_empty() {
            return false;
        }
        let step = self.frame_ms.max(1) as u64;
        (elapsed_ms / step) as usize >= self.frames.len().saturating_sub(1)
    }
}

/// A named animation the runtime can play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Animation {
    /// The resting face for an expression (replaces the procedural render).
    Expression(BallExpression),
    /// Poked three or four times.
    Annoyed,
    /// Low battery.
    Tired,
    /// Critical battery.
    Dying,
}

impl Animation {
    /// Animation for a personality reaction that is not covered by an
    /// expression sprite.
    pub fn for_reaction(reaction: PersonaReaction) -> Option<Self> {
        match reaction {
            PersonaReaction::Annoyed => Some(Self::Annoyed),
            // Curious, Puzzled and Angry have strong upstream expressions.
            PersonaReaction::Curious | PersonaReaction::Puzzled | PersonaReaction::Angry => None,
        }
    }

    /// Haptic pattern for an animation that replaces a reaction, if any.
    ///
    /// Kept here so a sprite override does not lose the buzz that the
    /// procedural reaction would have produced.
    pub fn haptic(self) -> Option<HapticPattern> {
        match self {
            Self::Annoyed => Some(HapticPattern::Confused),
            Self::Expression(_) | Self::Tired | Self::Dying => None,
        }
    }
}

/// The sprites the firmware has for each animation.
///
/// A `None` slot means "draw procedurally", which is every slot today.
pub struct SpriteBank {
    slots: [Option<&'static Sprite>; Animation::COUNT],
}

impl Animation {
    /// Number of animation slots in a bank.
    pub const COUNT: usize = 4 + BallExpression::ALL.len();

    /// Stable index for the bank array.
    const fn index(self) -> usize {
        match self {
            Self::Annoyed => 0,
            Self::Tired => 1,
            Self::Dying => 2,
            Self::Expression(_) => 3,
        }
    }
}

impl SpriteBank {
    /// An empty bank: every animation is drawn procedurally.
    pub const EMPTY: Self = Self {
        slots: [None; Animation::COUNT],
    };

    /// Install a sprite for one animation.
    pub fn set(&mut self, animation: Animation, sprite: &'static Sprite) {
        self.slots[animation.index()] = Some(sprite);
    }

    /// The sprite for one animation, if any.
    pub fn get(&self, animation: Animation) -> Option<&'static Sprite> {
        self.slots[animation.index()]
    }

    /// Whether any sprite is installed.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

impl Default for SpriteBank {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Draw one sprite frame into the canvas, nearest-neighbour scaled.
///
/// `canvas` is `canvas_size * canvas_size` RGB565; it is cleared to black
/// first, so a sprite with transparent margins stays surrounded by the panel
/// background. Extra pixels in `canvas` are left untouched.
pub fn blit_scaled(frame: &[u16], canvas: &mut [u16], canvas_size: usize, sprite_size: usize) {
    let pixels = canvas_size * canvas_size;
    if canvas.len() < pixels || sprite_size == 0 || frame.len() < sprite_size * sprite_size {
        return;
    }
    canvas[..pixels].fill(0);

    // Integer source index per destination pixel, precomputed per axis so the
    // inner loop is a table lookup rather than a multiply.
    // A 240-wide canvas with 48 KiB of stack is fine on the S31; the table is
    // fixed-size so no allocation is needed.
    let mut map = [0u16; crate::raster::CANVAS_SIZE];
    let map = &mut map[..canvas_size];
    for (dst, slot) in map.iter_mut().enumerate() {
        *slot = ((dst * sprite_size) / canvas_size) as u16;
    }

    for y in 0..canvas_size {
        let src_row = map[y] as usize * sprite_size;
        let dst_row = y * canvas_size;
        for x in 0..canvas_size {
            canvas[dst_row + x] = frame[src_row + map[x] as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ONE_FRAME: [u16; 4] = [0x1111, 0x2222, 0x3333, 0x4444];
    static FRAMES_A: [&[u16]; 2] = [&ONE_FRAME, &ONE_FRAME];
    static LOOP: Sprite = Sprite {
        size: 2,
        frame_ms: 100,
        looping: true,
        frames: &FRAMES_A,
    };
    static ONCE: Sprite = Sprite {
        size: 2,
        frame_ms: 100,
        looping: false,
        frames: &FRAMES_A,
    };

    #[test]
    fn a_looping_sprite_wraps() {
        assert_eq!(LOOP.frame_at(0), Some(0));
        assert_eq!(LOOP.frame_at(99), Some(0));
        assert_eq!(LOOP.frame_at(100), Some(1));
        assert_eq!(LOOP.frame_at(200), Some(0));
        assert!(!LOOP.finished(10_000));
    }

    #[test]
    fn a_one_shot_holds_its_last_frame() {
        assert_eq!(ONCE.frame_at(0), Some(0));
        assert_eq!(ONCE.frame_at(100), Some(1));
        assert_eq!(ONCE.frame_at(10_000), Some(1), "the last pose is held");
        assert!(ONCE.finished(100));
        assert!(!ONCE.finished(50));
    }

    #[test]
    fn an_empty_sprite_is_safe() {
        static EMPTY: Sprite = Sprite {
            size: 2,
            frame_ms: 10,
            looping: true,
            frames: &[],
        };
        assert_eq!(EMPTY.frame_at(0), None);
        assert!(!EMPTY.finished(1_000));
    }

    #[test]
    fn a_bank_finds_installed_sprites() {
        let mut bank = SpriteBank::EMPTY;
        assert!(bank.is_empty());
        assert!(bank.get(Animation::Annoyed).is_none());

        bank.set(Animation::Annoyed, &LOOP);
        assert!(!bank.is_empty());
        assert_eq!(
            bank.get(Animation::Annoyed).map(Sprite::frame_count),
            Some(2)
        );
        assert!(bank.get(Animation::Tired).is_none());
    }

    #[test]
    fn reactions_map_to_their_animations() {
        assert_eq!(
            Animation::for_reaction(PersonaReaction::Angry),
            None,
            "anger keeps the procedural expression"
        );
        assert_eq!(
            Animation::for_reaction(PersonaReaction::Annoyed),
            Some(Animation::Annoyed)
        );
    }

    #[test]
    fn blit_scales_nearest_neighbour() {
        // A 2x2 checkerboard scaled to 4x4 keeps crisp quadrants.
        let mut canvas = [0xFFFFu16; 16];
        blit_scaled(&ONE_FRAME, &mut canvas, 4, 2);
        assert_eq!(canvas[0], 0x1111);
        assert_eq!(canvas[1], 0x1111);
        assert_eq!(canvas[2], 0x2222);
        assert_eq!(canvas[3], 0x2222);
        assert_eq!(canvas[4], 0x1111);
        assert_eq!(canvas[5], 0x1111);
        assert_eq!(canvas[8], 0x3333);
        assert_eq!(canvas[15], 0x4444);
    }

    #[test]
    fn blit_clears_the_canvas_outside_the_sprite() {
        let mut canvas = [0xFFFFu16; 16];
        let mut frame = [0u16; 16];
        frame[0] = 0xABCD;
        blit_scaled(&frame, &mut canvas, 4, 4);
        assert_eq!(canvas[0], 0xABCD);
        assert!(canvas[1..].iter().all(|&pixel| pixel == 0));
    }

    #[test]
    fn blit_ignores_mismatched_sizes() {
        let mut canvas = [0xFFFFu16; 4];
        blit_scaled(&ONE_FRAME, &mut canvas, 4, 2); // canvas too small
        assert_eq!(canvas, [0xFFFF; 4], "a short canvas is left untouched");
    }
}
