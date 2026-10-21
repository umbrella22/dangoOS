//! Replaceable character skins.
//!
//! The ball is a body outline plus a pair of eyes. A skin changes what the body
//! *is* without touching the expression engine: the same 16 expressions, gaze,
//! blinking and touch personality drive every character.
//!
//! # Two pipelines
//!
//! 1. **Procedural (implemented here).** A skin is an optional body color plus
//!    a list of accessory polygons in the upstream source space
//!    (`viewBox -15 -15 259 259`). Cat and fox ears are triangles, so they cost
//!    a few hundred bytes of flash and scale with the body to any canvas size.
//!    This is what the firmware ships today.
//! 2. **Sprite (asset pipeline).** A pixel-art character (fur tufts, a hat, a
//!    tail) arrives as a PNG frame set, is converted to RGB565 at build time,
//!    and is blitted behind the eyes. That pipeline needs real art, so it is
//!    specified in `docs/ASSET_REQUESTS.md` and not implemented yet.
//!
//! # Adding a character
//!
//! Add one `Skin` constant with its accessory rings, append it to [`ALL`], and
//! the runtime can select it by name. Accessory points use the same coordinate
//! space as [`crate::data::BODY_RING`], so an artist can draw them directly on
//! top of the existing body outline.

use crate::types::{Point, Rgb};

/// Maximum points in one accessory polygon.
pub const ACCESSORY_POINTS: usize = 16;

/// Where an accessory is drawn relative to the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Behind the body: ears, tails, wings. The body hides the base.
    Behind,
    /// In front of the body and eyes: glasses, a gem, a nose.
    Front,
}

/// One polygon accessory.
#[derive(Clone, Copy, Debug)]
pub struct Accessory {
    pub points: [Point; ACCESSORY_POINTS],
    /// Number of valid points in `points`.
    pub len: u8,
    pub color: Rgb,
    pub layer: Layer,
}

impl Accessory {
    /// The valid prefix of `points`.
    pub fn ring(&self) -> &[Point] {
        &self.points[..(self.len as usize).min(ACCESSORY_POINTS)]
    }
}

/// A complete character appearance.
#[derive(Clone, Copy, Debug)]
pub struct Skin {
    pub name: &'static str,
    /// `None` keeps the expression's own body color.
    pub body_color: Option<Rgb>,
    pub accessories: &'static [Accessory],
}

impl Skin {
    /// Whether this skin changes anything.
    pub fn is_plain(&self) -> bool {
        self.body_color.is_none() && self.accessories.is_empty()
    }
}

/// The original blob: no accessories, the expression's own body color.
pub static BLOB: Skin = Skin {
    name: "blob",
    body_color: None,
    accessories: &[],
};

/// Build an accessory from a point list at compile time.
const fn accessory(points: &[(f32, f32)], color: u32, layer: Layer) -> Accessory {
    let mut out = [Point::ZERO; ACCESSORY_POINTS];
    let mut index = 0;
    while index < points.len() && index < ACCESSORY_POINTS {
        out[index] = Point {
            x: points[index].0,
            y: points[index].1,
        };
        index += 1;
    }
    Accessory {
        points: out,
        len: index as u8,
        color: Rgb::from_hex(color),
        layer,
    }
}

/// The head spans `0..=228.54` with its center at `114.2705` and a radius of
/// roughly `114`; mirroring a point is `x' = 228.54 - x`, so an artist only
/// draws the left ear.
///
/// The blob fills almost the whole viewBox, so an ear is only visible where it
/// leaves the head circle. Each ear therefore has two base points *inside* the
/// circle (hidden behind the body) and one apex outside it.
const fn mirror_x(x: f32) -> f32 {
    228.54 - x
}

// ---- cat ------------------------------------------------------------------

/// Cat left ear: a short triangle leaving the head at ~150 deg and pointing
/// toward the top-left corner of the viewBox.
const CAT_EAR_LEFT: [(f32, f32); 3] = [(30.4, 69.7), (6.0, -10.0), (88.1, 22.9)];
const CAT_EAR_RIGHT: [(f32, f32); 3] = [
    (mirror_x(30.4), 69.7),
    (mirror_x(6.0), -10.0),
    (mirror_x(88.1), 22.9),
];
/// Cat inner ear: the same apex pulled in by ~10 units and a narrower base.
const CAT_INNER_LEFT: [(f32, f32); 3] = [(37.0, 58.0), (17.0, -1.0), (78.0, 25.0)];
const CAT_INNER_RIGHT: [(f32, f32); 3] = [
    (mirror_x(37.0), 58.0),
    (mirror_x(17.0), -1.0),
    (mirror_x(78.0), 25.0),
];

/// Cat character: pointed ears with a warm inner ear.
pub static CAT: Skin = Skin {
    name: "cat",
    body_color: None,
    accessories: &[
        accessory(&CAT_EAR_LEFT, 0xf3f0ea, Layer::Behind),
        accessory(&CAT_EAR_RIGHT, 0xf3f0ea, Layer::Behind),
        accessory(&CAT_INNER_LEFT, 0xd9a3a0, Layer::Behind),
        accessory(&CAT_INNER_RIGHT, 0xd9a3a0, Layer::Behind),
    ],
};

// ---- fox ------------------------------------------------------------------

/// Fox ears are taller and narrower than the cat's.
const FOX_EAR_LEFT: [(f32, f32); 3] = [(33.0, 73.0), (4.0, -14.0), (90.0, 20.0)];
const FOX_EAR_RIGHT: [(f32, f32); 3] = [
    (mirror_x(33.0), 73.0),
    (mirror_x(4.0), -14.0),
    (mirror_x(90.0), 20.0),
];
const FOX_INNER_LEFT: [(f32, f32); 3] = [(40.0, 62.0), (13.0, -4.0), (80.0, 22.0)];
const FOX_INNER_RIGHT: [(f32, f32); 3] = [
    (mirror_x(40.0), 62.0),
    (mirror_x(13.0), -4.0),
    (mirror_x(80.0), 22.0),
];

/// Fox character: tall orange ears with cream inner ears.
pub static FOX: Skin = Skin {
    name: "fox",
    body_color: Some(Rgb::from_hex(0xe8a05a)),
    accessories: &[
        accessory(&FOX_EAR_LEFT, 0xe8a05a, Layer::Behind),
        accessory(&FOX_EAR_RIGHT, 0xe8a05a, Layer::Behind),
        accessory(&FOX_INNER_LEFT, 0xf6e7cf, Layer::Behind),
        accessory(&FOX_INNER_RIGHT, 0xf6e7cf, Layer::Behind),
    ],
};

/// Every skin the firmware can select.
pub static ALL: [&Skin; 3] = [&BLOB, &CAT, &FOX];

/// Look a skin up by name; unknown names fall back to [`BLOB`].
pub fn by_name(name: &str) -> &'static Skin {
    let mut index = 0;
    while index < ALL.len() {
        if ALL[index].name == name {
            return ALL[index];
        }
        index += 1;
    }
    &BLOB
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skin_has_a_valid_accessory_ring() {
        for skin in ALL {
            for item in skin.accessories {
                let ring = item.ring();
                assert!(
                    ring.len() >= 3,
                    "skin {} accessory has {} points",
                    skin.name,
                    ring.len()
                );
                for point in ring {
                    assert!(
                        point.x > -30.0 && point.x < 260.0 && point.y > -30.0 && point.y < 260.0,
                        "skin {} accessory point outside the body space: ({}, {})",
                        skin.name,
                        point.x,
                        point.y
                    );
                }
            }
        }
    }

    #[test]
    fn ears_leave_the_head_circle() {
        // The blob is a circle of radius ~114 around (114.27, 114.27). An ear
        // that stays inside it is invisible behind the body, so every ear must
        // have an apex outside the circle.
        const CX: f32 = 114.2705;
        const CY: f32 = 114.2705;
        const R: f32 = 114.2705;
        for skin in [&CAT, &FOX] {
            for item in &skin.accessories[..2] {
                let apex = item.ring()[1];
                let dx = apex.x - CX;
                let dy = apex.y - CY;
                let distance = (dx * dx + dy * dy).sqrt();
                assert!(
                    distance > R * 1.15,
                    "skin {} ear apex is inside the head circle (distance {distance})",
                    skin.name
                );
            }
        }
    }

    #[test]
    fn the_blob_is_plain_and_lookups_are_safe() {
        assert!(BLOB.is_plain());
        assert!(!CAT.is_plain());
        assert_eq!(by_name("cat").name, "cat");
        assert_eq!(by_name("fox").name, "fox");
        assert_eq!(by_name("nope").name, "blob");
    }

    #[test]
    fn ears_are_symmetric_around_the_head_center() {
        for (left, right) in [
            (CAT.accessories[0].ring(), CAT.accessories[1].ring()),
            (FOX.accessories[0].ring(), FOX.accessories[1].ring()),
        ] {
            assert_eq!(left.len(), right.len());
            for (l, r) in left.iter().zip(right.iter()) {
                assert!(
                    (mirror_x(l.x) - r.x).abs() < 0.001,
                    "mirrored ear point mismatch: {} vs {}",
                    l.x,
                    r.x
                );
                assert_eq!(l.y, r.y);
            }
        }
    }

    #[test]
    fn every_skin_name_is_unique() {
        for (index, skin) in ALL.iter().enumerate() {
            for other in &ALL[index + 1..] {
                assert_ne!(skin.name, other.name);
            }
        }
    }
}
