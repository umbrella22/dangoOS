//! Validated, owned skin resources. No file-backed pointer reaches the renderer.
use alloc::{boxed::Box, rc::Rc, string::String, vec::Vec};
use miniz_oxide::inflate::{
    core::{decompress, inflate_flags, DecompressorOxide},
    TINFLStatus,
};
use {
    crate::companion::render::data, crate::companion::render::FoxPose,
    crate::companion::render::Img, crate::companion::render::Pose, crate::companion::render::H,
    crate::companion::render::W,
};

pub const MAGIC: &[u8; 8] = b"MSKIN01\0";
pub const HEADER: usize = 36;
pub const MAX_BYTES: usize = 1024 * 1024;
/// A legacy whole-package wrapper retains the original format's decoded
/// resource limit. Version 2 leaves inactive images compressed in memory.
pub const MAX_DECODED_BYTES: usize = MAX_BYTES;
const COMPRESSED_MAGIC: &[u8; 8] = b"MSKINZ1\0";
pub const POSES: usize = 25;
pub const GROUPS: usize = 11;

#[derive(Clone, Copy, Debug)]
pub struct RigHand {
    pub img: Img,
    /// Pivot in the hand image, in source pixels.
    pub pivot: [i16; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct BalanceRig {
    pub body: Img,
    pub hands: [RigHand; 2],
    /// v4: effort, dizzy, weary, startled; transparent face patches.
    /// v3 retains its release and landing faces in the middle slots.
    pub faces: [Img; 4],
    pub articulation: Option<RigArticulation>,
}

#[derive(Clone, Copy, Debug)]
pub struct RigArticulation {
    pub ears: [RigHand; 2],
    pub blink: Img,
}

// A non-inlined return lets Box::write initialize the large Huffman tables
// directly in its heap allocation. Box::default() leaves a 10 KiB temporary
// on the RISC-V stack, even in the optimized firmware.
#[inline(never)]
fn new_decompressor() -> DecompressorOxide {
    DecompressorOxide::new()
}

/// Reuse one lazily allocated state while validating a package. Legacy RLE
/// packages need no decoder. Neither this state nor decoded pixels are retained
/// after validation; the renderer continues to own its separate working buffers.
#[derive(Default)]
struct SkinDecoder(Option<Box<DecompressorOxide>>);
impl SkinDecoder {
    fn decompress(&mut self, input: &[u8], limit: usize) -> Result<Vec<u8>, TINFLStatus> {
        let state = self
            .0
            .get_or_insert_with(|| Box::write(Box::new_uninit(), new_decompressor()));
        state.init();
        let mut output = alloc::vec![0; limit];
        let (status, _, written) = decompress(
            state,
            input,
            &mut output,
            0,
            inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
                | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
        );
        if status != TINFLStatus::Done {
            return Err(status);
        }
        output.truncate(written);
        Ok(output)
    }
}

#[derive(Debug)]
pub struct OwnedPose {
    pub base: Img,
    pub groups: [Vec<Img>; GROUPS],
}
/// Borrowed view shared by the static placeholder and validated packages.
pub struct PoseView<'a> {
    pub base: Img,
    pub blink: &'a [Img],
    pub mouth_closed: &'a [Img],
    pub mouth_wide: &'a [Img],
    pub ear: &'a [Img],
    pub gaze_left: &'a [Img],
    pub gaze_right: &'a [Img],
    pub symbols: &'a [Img],
    pub tremble: &'a [Img],
    pub spiral: [&'a [Img]; 3],
}
impl<'a> From<&'a Pose> for PoseView<'a> {
    fn from(p: &'a Pose) -> Self {
        Self {
            base: p.base,
            blink: p.blink,
            mouth_closed: p.mouth_closed,
            mouth_wide: p.mouth_wide,
            ear: p.ear,
            gaze_left: p.gaze_left,
            gaze_right: p.gaze_right,
            symbols: p.symbols,
            tremble: p.tremble,
            spiral: p.spiral,
        }
    }
}
#[derive(Debug)]
struct Package {
    palette: Vec<u16>,
    poses: Vec<OwnedPose>,
    bytes: Vec<u8>,
    blob_start: usize,
    indexed_zlib: bool,
    balance: Option<BalanceRig>,
}
#[derive(Debug)]
pub struct Skin {
    package: Option<Package>,
}
impl Skin {
    pub fn balance_rig(&self) -> Option<BalanceRig> {
        self.package.as_ref().and_then(|p| p.balance)
    }
    pub(crate) fn same_resources(a: &Rc<Self>, b: &Rc<Self>) -> bool {
        Rc::ptr_eq(a, b) || (a.package.is_none() && b.package.is_none())
    }
    pub fn builtin() -> Rc<Self> {
        Rc::new(Self { package: None })
    }
    pub fn palette(&self) -> &[u16] {
        self.package.as_ref().map_or(&data::PALETTE, |p| &p.palette)
    }
    pub fn blob(&self) -> &[u8] {
        self.package
            .as_ref()
            .map_or(data::BLOB, |p| &p.bytes[p.blob_start..])
    }
    pub(crate) fn indexed_zlib(&self) -> bool {
        self.package.as_ref().is_some_and(|p| p.indexed_zlib)
    }
    pub(crate) fn decode_indexed(&self, img: &Img, pixels: &mut [u8]) {
        let source = &self.blob()[img.off as usize..img.off as usize + img.len as usize];
        let written = miniz_oxide::inflate::decompress_slice_iter_to_slice(
            pixels,
            core::iter::once(source),
            true,
            false,
        );
        // Both stream and output dimensions were validated before this
        // immutable resource was published. A render cannot change either.
        assert_eq!(written, Ok(pixels.len()));
    }
    pub fn pose(&self, pose: FoxPose) -> PoseView<'_> {
        let Some(package) = &self.package else {
            return (&data::PLACEHOLDER_POSE).into();
        };
        let p = package
            .poses
            .get(pose as usize)
            .unwrap_or_else(|| &package.poses[pose.legacy_fallback() as usize]);
        let g = &p.groups;
        PoseView {
            base: p.base,
            blink: &g[0],
            mouth_closed: &g[1],
            mouth_wide: &g[2],
            ear: &g[3],
            gaze_left: &g[4],
            gaze_right: &g[5],
            symbols: &g[6],
            tremble: &g[7],
            spiral: [&g[8], &g[9], &g[10]],
        }
    }
    pub fn parse(bytes: Vec<u8>) -> Result<Rc<Self>, String> {
        if bytes.len() > MAX_BYTES {
            return Err("skin transfer size".into());
        }
        let mut decoder = SkinDecoder::default();
        let bytes = if bytes.starts_with(COMPRESSED_MAGIC) {
            if bytes.len() < 12 {
                return Err("skin compressed header".into());
            }
            let size = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
            if !(HEADER..=MAX_DECODED_BYTES).contains(&size) {
                return Err("skin decoded size".into());
            }
            let decoded = decoder
                .decompress(&bytes[12..], size)
                .map_err(|_| String::from("skin compressed data"))?;
            if decoded.len() != size {
                return Err("skin decoded length".into());
            }
            decoded
        } else {
            bytes
        };
        if bytes.len() < HEADER || &bytes[..8] != MAGIC {
            return Err("skin binary magic/size".into());
        }
        let mut r = Reader { b: &bytes, at: 8 };
        let version = r.u32()?;
        if ![1, 2, 3, 4].contains(&version) || r.u16()? as usize != W || r.u16()? as usize != H {
            return Err("skin binary version/canvas".into());
        }
        let colors = r.u16()? as usize;
        let poses = r.u16()? as usize;
        let meta = r.u32()? as usize;
        let blob_len = r.u32()? as usize;
        let crc = r.u32()?;
        let reserved = r.u32()?;
        if colors == 0
            || colors > 255
            || ![20, POSES].contains(&poses)
            || reserved != 0
            || meta > 65536
            || HEADER
                .checked_add(meta)
                .and_then(|n| n.checked_add(blob_len))
                != Some(bytes.len())
        {
            return Err("skin binary table/length".into());
        }
        if crate::formats::crc32::crc32(&bytes[HEADER..]) != crc {
            return Err("skin binary checksum".into());
        }
        let end = HEADER + meta;
        r.b = &bytes[..end];
        let palette = (0..colors)
            .map(|_| r.u16())
            .collect::<Result<Vec<_>, _>>()?;
        let mut table = Vec::with_capacity(poses);
        for id in 0..poses {
            if r.u16()? as usize != id {
                return Err("skin missing/duplicate/out-of-order pose".into());
            }
            let base = r.img()?;
            let mut groups: [Vec<Img>; GROUPS] = core::array::from_fn(|_| Vec::new());
            for g in &mut groups {
                let n = r.u16()? as usize;
                if n.checked_mul(20)
                    .is_none_or(|size| size > r.b.len().saturating_sub(r.at))
                {
                    return Err("skin truncated layer table".into());
                }
                for _ in 0..n {
                    g.push(r.img()?);
                }
            }
            validate_layers(id, &groups)?;
            if base.x != 0
                || base.y != 0
                || base.w as usize != W
                || base.h as usize != H
                || base.over_bg
            {
                return Err("skin base dimensions".into());
            }
            validate_img(
                &base,
                &bytes[end..],
                colors,
                true,
                version >= 2,
                &mut decoder,
            )?;
            for img in groups.iter().flatten() {
                validate_img(
                    img,
                    &bytes[end..],
                    colors,
                    false,
                    version >= 2,
                    &mut decoder,
                )?;
            }
            table.push(OwnedPose { base, groups });
        }
        // v3 appends one optional-motion rig after the unchanged pose table.
        // Existing v1/v2 characters remain usable without articulated paws.
        let balance = if version >= 3 {
            let body = r.img()?;
            if body.x != 0
                || body.y != 0
                || body.w as usize != W
                || body.h as usize != H
                || body.over_bg
            {
                return Err("skin balance body dimensions".into());
            }
            validate_img(
                &body,
                &bytes[end..],
                colors,
                version == 3,
                true,
                &mut decoder,
            )?;
            let hands = [
                r.rig_part(&bytes[end..], colors, &mut decoder)?,
                r.rig_part(&bytes[end..], colors, &mut decoder)?,
            ];
            let mut faces = [body; 4];
            for face in &mut faces {
                *face = r.img()?;
                if face.over_bg {
                    return Err("skin balance face flags".into());
                }
                validate_img(face, &bytes[end..], colors, false, true, &mut decoder)?;
            }
            let articulation = if version == 4 {
                let ears = [
                    r.rig_part(&bytes[end..], colors, &mut decoder)?,
                    r.rig_part(&bytes[end..], colors, &mut decoder)?,
                ];
                let blink = r.img()?;
                if blink.over_bg {
                    return Err("skin balance blink flags".into());
                }
                validate_img(&blink, &bytes[end..], colors, false, true, &mut decoder)?;
                Some(RigArticulation { ears, blink })
            } else {
                None
            };
            Some(BalanceRig {
                body,
                hands,
                faces,
                articulation,
            })
        } else {
            None
        };
        if r.at != end {
            return Err("skin trailing metadata".into());
        }
        // All metadata and RLE have been checked before the resource can exist.
        Ok(Rc::new(Self {
            package: Some(Package {
                palette,
                poses: table,
                bytes,
                blob_start: end,
                indexed_zlib: version >= 2,
                balance,
            }),
        }))
    }
}
// Match the animator's simultaneous combinations, independently for the
// under/over buffers. Mutually exclusive mouth/spiral/gaze frames share capacity.
fn validate_layers(id: usize, g: &[Vec<Img>; GROUPS]) -> Result<(), String> {
    let blink = g[0].len();
    let symbols = g[6].len();
    let over = g[6].iter().filter(|i| i.over_bg).count();
    let under = symbols - over;
    let (u, o) = match id {
        0 => (blink.max(g[3].len()).max(g[4].len()).max(g[5].len()), 0), // Idle
        18 => (blink.max(g[4].len()).max(g[5].len()), 0),                // Peering
        10 => (blink + g[3].len(), 0),                                   // Listening
        11 => (blink, symbols.min(4)), // Thinking lights at most four dots
        12 => (blink + g[1].len().max(g[2].len()), 0), // Speaking
        1 | 4 => (blink, symbols),     // Sleep / Puzzled
        7 => (blink + under, over + g[7].len()), // Panic
        17 => (
            blink,
            (0..4)
                .map(|phase| {
                    g[6].iter()
                        .enumerate()
                        .filter(|(n, _)| *n == 0 || (phase + n * 7 / 4) % 4 < 3)
                        .count()
                })
                .max()
                .unwrap_or(0),
        ),
        5 => (
            blink + under + g[8].len().max(g[9].len()).max(g[10].len()),
            over,
        ), // Confused
        8 => (blink + symbols, 0),   // Angry
        16 => (blink + under, over), // Annoyed
        _ => (blink, 0),
    };
    if u > crate::companion::render::scene::MAX_LAYERS
        || o > crate::companion::render::scene::MAX_LAYERS
    {
        return Err("skin simultaneous layer capacity".into());
    }
    Ok(())
}

fn validate_img(
    img: &Img,
    blob: &[u8],
    colors: usize,
    opaque: bool,
    indexed_zlib: bool,
    decoder: &mut SkinDecoder,
) -> Result<(), String> {
    if img.w == 0
        || img.h == 0
        || img.x < 0
        || img.y < 0
        || img.x as usize + img.w as usize > W
        || img.y as usize + img.h as usize > H
    {
        return Err("skin image bounds".into());
    }
    let start = img.off as usize;
    let end = start
        .checked_add(img.len as usize)
        .ok_or("skin RLE offset overflow")?;
    let b = blob.get(start..end).ok_or("skin RLE outside blob")?;
    if indexed_zlib {
        let size = img.w as usize * img.h as usize;
        let pixels = decoder
            .decompress(b, size)
            .map_err(|_| String::from("skin indexed zlib data"))?;
        if pixels.len() != size
            || pixels
                .iter()
                .any(|&v| (v as usize >= colors && v != 255) || (opaque && v == 255))
        {
            return Err("skin indexed length/palette/opacity".into());
        }
        return Ok(());
    }
    if b.is_empty() || b.len() % 2 != 0 {
        return Err("skin RLE length".into());
    }
    let mut x = 0;
    let mut rows = 0;
    for pair in b.chunks_exact(2) {
        if pair[0] == 0
            || (pair[1] as usize >= colors && pair[1] != 255)
            || (opaque && pair[1] == 255)
        {
            return Err("skin RLE count/palette/opacity".into());
        }
        x += pair[0] as usize;
        if x > img.w as usize {
            return Err("skin RLE crosses row".into());
        }
        if x == img.w as usize {
            x = 0;
            rows += 1;
        }
        if rows > img.h as usize {
            return Err("skin RLE excess rows".into());
        }
    }
    if x != 0 || rows != img.h as usize {
        return Err("skin RLE incomplete image".into());
    }
    Ok(())
}
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

#[cfg(test)]
mod balance_tests {
    use super::*;

    #[test]
    fn every_shipped_character_has_a_valid_rig_and_legacy_skins_still_load() {
        for id in ["default", "eclipse", "normal", "black-blue"] {
            let path = alloc::format!(
                "{}/../../resources/skins/{id}/fox.bin",
                env!("CARGO_MANIFEST_DIR")
            );
            let skin = Skin::parse(std::fs::read(path).unwrap()).unwrap();
            let rig = skin.balance_rig().expect(id);
            assert!(rig.articulation.is_some(), "{id}: articulated skin");
            assert_eq!((rig.body.w as usize, rig.body.h as usize), (W, H));
            for hand in rig.hands {
                assert!(hand.img.w < 80 && hand.img.h < 80);
                let mut pixels = alloc::vec![0; hand.img.w as usize * hand.img.h as usize];
                skin.decode_indexed(&hand.img, &mut pixels);
                assert!(pixels.contains(&255), "{id}: hand needs transparency");
                assert!(pixels.iter().any(|&v| v != 255));
            }
        }
        assert!(Skin::parse(fixture(0)).unwrap().balance_rig().is_none());
    }

    #[test]
    fn corrupt_hand_pivots_are_rejected_before_publishing_a_skin() {
        let mut bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/skins/default/fox.bin"
        ))
        .to_vec();
        let meta = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let rig_bytes = if version == 4 { 216 } else { 148 };
        let last_pivot = HEADER + meta - rig_bytes + 64;
        bytes[last_pivot..last_pivot + 2].copy_from_slice(&u16::MAX.to_le_bytes());
        let crc = crate::formats::crc32::crc32(&bytes[HEADER..]);
        bytes[28..32].copy_from_slice(&crc.to_le_bytes());
        assert!(Skin::parse(bytes).unwrap_err().contains("hand pivot"));
    }

    #[test]
    fn articulated_skins_keep_v2_v3_readable_and_reject_invalid_extra_pivots() {
        let original = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/skins/default/fox.bin"
        ))
        .to_vec();
        let meta = u32::from_le_bytes(original[20..24].try_into().unwrap()) as usize;
        let start = HEADER + meta - 216;
        for offset in [168, 192] {
            let mut bad = original.clone();
            bad[start + offset..start + offset + 2].copy_from_slice(&u16::MAX.to_le_bytes());
            fix_crc(&mut bad);
            assert!(Skin::parse(bad).unwrap_err().contains("hand pivot"));
        }
        for version in [2u32, 3] {
            let mut legacy = original.clone();
            let colors = u16::from_le_bytes(legacy[16..18].try_into().unwrap()) as usize;
            let base = legacy[HEADER + colors * 2 + 2..HEADER + colors * 2 + 22].to_vec();
            // v3 required an opaque full-canvas body. Use the original idle
            // body while removing the v4-only metadata; unused blob bytes are
            // allowed in all three formats.
            legacy[start..start + 20].copy_from_slice(&base);
            let removed = if version == 2 { 216 } else { 68 };
            legacy.drain(HEADER + meta - removed..HEADER + meta);
            legacy[8..12].copy_from_slice(&version.to_le_bytes());
            legacy[20..24].copy_from_slice(&((meta - removed) as u32).to_le_bytes());
            fix_crc(&mut legacy);
            let skin = Skin::parse(legacy).unwrap();
            assert_eq!(skin.balance_rig().is_some(), version == 3);
            assert!(skin.balance_rig().is_none_or(|r| r.articulation.is_none()));
        }
    }

    #[test]
    fn holding_one_side_only_gently_stretches_the_leading_mitten() {
        use crate::companion::{
            behavior::balance::{BalanceStage, BalanceVisual},
            render::{FoxAnimator, FoxInput},
        };
        let animator = FoxAnimator::with_skin(FoxPose::Idle, reference());
        for side in [-1.0, 1.0] {
            let scene = animator.scene(FoxInput {
                balance: BalanceVisual {
                    active: true,
                    stage: BalanceStage::Struggling,
                    grip: 1.0,
                    drag: 1.0,
                    reach: 1.0,
                    hold_side: side,
                    ..BalanceVisual::REST
                },
                ..FoxInput::REST
            });
            let hands = scene.rig.unwrap().hands;
            let lead = if side < 0.0 { 0 } else { 1 };
            assert!(hands[lead].scale_y > hands[1 - lead].scale_y);
            for hand in hands {
                assert!(
                    hand.scale_y / hand.scale_x < 1.32,
                    "mitten keeps its round silhouette"
                );
            }
            assert!(
                hands[lead].dy < hands[1 - lead].dy,
                "one hand catches the side while the other follows"
            );
        }
    }

    #[test]
    fn completed_rotation_does_not_flash_the_old_confused_pose() {
        use crate::companion::{
            behavior::{
                affect::{EventKind, EventSource, SensorEvent},
                balance::{BalanceFace, BalanceVisual},
            },
            render::{FoxAnimator, FoxInput},
        };
        use crate::{Expression, FoxControl, Runtime};
        for use_brain in [false, true] {
            let control = FoxControl::new(Expression::Idle);
            let mut runtime = Runtime::new(0, Rc::clone(&control));
            if use_brain {
                runtime
                    .enable_brain(
                        crate::companion::brain::DEFAULT_SOURCE,
                        0,
                        &app_host::GLOBAL_ALLOCATOR,
                        None,
                    )
                    .unwrap();
            }
            let mut animator = FoxAnimator::with_skin(FoxPose::Idle, reference());
            let mut recovered_while_affect_was_still_dizzy = false;
            for at in (20u64..=4800).step_by(20) {
                let moving = at.saturating_sub(500).min(900);
                let angle = (moving as f32 * 0.6).to_radians();
                for kind in [
                    EventKind::Accel {
                        ax_mg: (angle.sin() * 1000.0) as i16,
                        ay_mg: (angle.cos() * 1000.0) as i16,
                        az_mg: 0,
                    },
                    EventKind::Gyro {
                        gx_dps: 0,
                        gy_dps: 0,
                        gz_dps: if at > 500 && moving < 900 { 600 } else { 0 },
                    },
                ] {
                    runtime.on_sensor(SensorEvent {
                        at_ms: at,
                        source: EventSource::Imu,
                        kind,
                    });
                }
                runtime.tick(at);
                animator.set_pose(control.expression().into());
                animator.advance(20);
                let balance = control.balance();
                let scene = animator.scene(FoxInput {
                    balance,
                    ..FoxInput::REST
                });
                if at > 3200 {
                    assert_eq!(balance.face, BalanceFace::Neutral);
                    assert_eq!(scene.pose, FoxPose::Idle);
                    assert!(
                        scene.rig.is_some(),
                        "same layered body through recovery and the Idle handoff"
                    );
                    if control.expression() == Expression::Confused {
                        recovered_while_affect_was_still_dizzy = true;
                        assert!(balance.rotation_reaction);
                    }
                }
            }
            assert!(recovered_while_affect_was_still_dizzy);
        }
        let unrelated = FoxAnimator::with_skin(FoxPose::Confused, reference()).scene(FoxInput {
            balance: BalanceVisual {
                active: true,
                ..BalanceVisual::REST
            },
            ..FoxInput::REST
        });
        assert_eq!(unrelated.pose, FoxPose::Confused);
        assert!(unrelated.rig.is_none());
    }
}
impl Reader<'_> {
    fn rig_part(
        &mut self,
        blob: &[u8],
        colors: usize,
        decoder: &mut SkinDecoder,
    ) -> Result<RigHand, String> {
        let img = self.img()?;
        let pivot = [self.u16()? as i16, self.u16()? as i16];
        if img.over_bg
            || pivot[0] < 0
            || pivot[1] < 0
            || pivot[0] >= img.w as i16
            || pivot[1] >= img.h as i16
        {
            return Err("skin balance hand pivot".into());
        }
        validate_img(&img, blob, colors, false, true, decoder)?;
        Ok(RigHand { img, pivot })
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let b = self
            .b
            .get(self.at..self.at + N)
            .ok_or("skin truncated metadata")?;
        self.at += N;
        Ok(b.try_into().unwrap())
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn img(&mut self) -> Result<Img, String> {
        let x = self.u16()? as i16;
        let y = self.u16()? as i16;
        let w = self.u16()?;
        let h = self.u16()?;
        let off = self.u32()?;
        let len = self.u32()?;
        let flags = self.u32()?;
        if flags > 1 {
            return Err("skin image flags/reserved".into());
        }
        Ok(Img {
            x,
            y,
            w,
            h,
            off,
            len,
            over_bg: flags == 1,
        })
    }
}

/// Tests exercise real distributable art, independently of the placeholder.
#[cfg(test)]
pub(crate) fn reference() -> Rc<Skin> {
    Skin::parse(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/skins/default/fox.bin"
        ))
        .to_vec(),
    )
    .unwrap()
}

#[cfg(test)]
pub(crate) fn fixture(xor: u16) -> Vec<u8> {
    fixture_with(xor, |_, _| {})
}
// Keep v1 conversion independent of future artwork detail. Its decoded RLE
// budget is still 1 MiB, whereas current v4 art is stored per-image as zlib.
#[cfg(test)]
fn legacy_reference() -> Rc<Skin> {
    Skin::parse(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/legacy-default-v4.bin"
        ))
        .to_vec(),
    )
    .unwrap()
}
#[cfg(test)]
fn fixture_with(xor: u16, mut change: impl FnMut(usize, &mut [Vec<Img>; GROUPS])) -> Vec<u8> {
    fn img(b: &mut Vec<u8>, i: &Img) {
        for n in [i.x as u16, i.y as u16, i.w, i.h] {
            b.extend(n.to_le_bytes());
        }
        for n in [i.off, i.len, i.over_bg as u32] {
            b.extend(n.to_le_bytes());
        }
    }
    // Re-encode the fixed per-image zlib sample as legacy RLE so both
    // decoders and palette/layer mutations retain coverage after extraction.
    let source = legacy_reference();
    assert!(source.indexed_zlib());
    let mut blob = Vec::new();
    let mut rle = |original: Img| {
        let mut pixels = alloc::vec![0; original.w as usize * original.h as usize];
        source.decode_indexed(&original, &mut pixels);
        let off = blob.len();
        for row in pixels.chunks_exact(original.w as usize) {
            let mut x = 0;
            while x < row.len() {
                let mut end = x + 1;
                while end < row.len() && row[end] == row[x] && end - x < 255 {
                    end += 1;
                }
                blob.extend([(end - x) as u8, row[x]]);
                x = end;
            }
        }
        Img {
            off: off as u32,
            len: (blob.len() - off) as u32,
            ..original
        }
    };
    let mut meta = Vec::new();
    for &c in source.palette() {
        meta.extend((c ^ xor).to_le_bytes());
    }
    for (id, pose) in FoxPose::ALL.into_iter().enumerate() {
        let p = source.pose(pose);
        meta.extend((id as u16).to_le_bytes());
        img(&mut meta, &rle(p.base));
        let mut groups = [
            p.blink,
            p.mouth_closed,
            p.mouth_wide,
            p.ear,
            p.gaze_left,
            p.gaze_right,
            p.symbols,
            p.tremble,
            p.spiral[0],
            p.spiral[1],
            p.spiral[2],
        ]
        .map(|g| g.iter().copied().map(&mut rle).collect());
        change(id, &mut groups);
        for g in &groups {
            meta.extend((g.len() as u16).to_le_bytes());
            for i in g {
                img(&mut meta, i);
            }
        }
    }
    let mut out = Vec::from(*MAGIC);
    out.extend(1u32.to_le_bytes());
    for n in [
        W as u16,
        H as u16,
        source.palette().len() as u16,
        POSES as u16,
    ] {
        out.extend(n.to_le_bytes());
    }
    for n in [meta.len() as u32, blob.len() as u32, 0, 0] {
        out.extend(n.to_le_bytes());
    }
    out.extend(meta);
    out.extend(blob);
    fix_crc(&mut out);
    out
}
#[cfg(test)]
pub(crate) fn fix_crc(b: &mut [u8]) {
    let crc = crate::formats::crc32::crc32(&b[HEADER..]);
    b[28..32].copy_from_slice(&crc.to_le_bytes());
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use {
        crate::companion::render::FoxAnimator, crate::companion::render::FoxInput,
        crate::companion::render::FoxRenderer, crate::companion::render::Scene,
        crate::companion::render::PIXELS,
    };
    #[test]
    fn validation_decoder_reuses_state_and_enforces_limits_after_failed_streams() {
        let mut decoder = SkinDecoder::default();
        assert!(decoder.0.is_none());
        let plain: Vec<u8> = (0..16_384).map(|n| (n % 251) as u8).collect();
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9);
        assert_eq!(decoder.decompress(&compressed, plain.len()).unwrap(), plain);
        let state = (&**decoder.0.as_ref().unwrap()) as *const DecompressorOxide;
        assert!(decoder.decompress(&compressed, plain.len() - 1).is_err());
        assert!(decoder
            .decompress(&compressed[..compressed.len() - 1], plain.len())
            .is_err());
        let mut bad_checksum = compressed.clone();
        *bad_checksum.last_mut().unwrap() ^= 1;
        assert!(decoder.decompress(&bad_checksum, plain.len()).is_err());
        assert!(decoder.decompress(b"invalid", plain.len()).is_err());
        // Reset must recover from every previous failure, including a stream
        // that exhausted its output buffer, and retain the same allocation.
        for level in [0, 1, 9] {
            let encoded = miniz_oxide::deflate::compress_to_vec_zlib(&plain, level);
            assert_eq!(
                decoder.decompress(&encoded, plain.len() + 1).unwrap(),
                plain
            );
            assert_eq!(
                (&**decoder.0.as_ref().unwrap()) as *const DecompressorOxide,
                state
            );
        }
    }

    #[test]
    #[ignore = "requires private assets/dango-family artwork"]
    fn family_packages_keep_independent_colours_and_decode_only_one_image_at_a_time() {
        let mut signatures = Vec::new();
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dango-family");
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("import.json")).unwrap()).unwrap();
        let keys = [
            "idle",
            "sleep",
            "curious",
            "happy",
            "puzzled",
            "confused",
            "surprised",
            "panic",
            "angry",
            "satisfied",
            "listening",
            "thinking",
            "speaking",
            "error",
            "tired",
            "dying",
            "annoyed",
            "charging",
            "peering",
            "dazzled",
            "network-disconnected",
            "network-connected",
            "bluetooth-disconnected",
            "bluetooth-connected",
            "music-playing",
        ];
        for (index, id) in ["eclipse", "normal", "black-blue"].into_iter().enumerate() {
            let bytes = std::fs::read(root.join("skins").join(id).join("fox.bin")).unwrap();
            let skin = Skin::parse(bytes.clone()).unwrap();
            assert!(skin.indexed_zlib());
            assert_eq!(skin.package.as_ref().unwrap().bytes.len(), bytes.len());
            assert_eq!(skin.package.as_ref().unwrap().poses.len(), POSES);
            assert_eq!(skin.pose(FoxPose::Idle).blink.len(), 2);
            let mut renderer = FoxRenderer::new();
            let mut frame = vec![0; PIXELS];
            for pose in FoxPose::ALL {
                renderer.render(&Scene::with_skin(pose, Rc::clone(&skin)), &mut frame);
                assert!(frame.iter().any(|&v| v != 0));
                assert!(frame.iter().all(|v| skin.palette().contains(v)));
                let actual = crate::formats::crc32::crc32(
                    &frame
                        .iter()
                        .flat_map(|v| v.to_le_bytes())
                        .collect::<Vec<_>>(),
                );
                assert_eq!(
                    actual as u64,
                    receipt["characters"][index]["pose_rgb565_crc32"][keys[pose as usize]]
                        .as_u64()
                        .unwrap(),
                    "character {index}, pose {pose:?}"
                );
            }
            let scene = Scene::with_skin(FoxPose::Idle, Rc::clone(&skin));
            renderer.render(&scene, &mut frame);
            signatures.push(crate::formats::crc32::crc32(
                &frame
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            ));
            let reused = renderer.reused;
            renderer.render(&scene, &mut frame);
            assert_eq!(
                renderer.reused,
                reused + 1,
                "breathing does not inflate the base again"
            );
            // Corrupt compressed pixel data while preserving the outer CRC:
            // validation must still reject it before publishing a resource.
            let mut bad = bytes.to_vec();
            let start = HEADER + u32::from_le_bytes(bad[20..24].try_into().unwrap()) as usize;
            bad[start] = 0;
            fix_crc(&mut bad);
            assert!(Skin::parse(bad).is_err());
        }
        assert!(
            signatures[0] != signatures[1]
                && signatures[1] != signatures[2]
                && signatures[0] != signatures[2]
        );
    }
    #[test]
    fn legacy_twenty_pose_skin_keeps_its_own_art_for_new_business_expressions() {
        let mut bytes = fixture(0);
        let meta = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
        let end = HEADER + meta;
        // Locate the first twenty entries structurally: business poses can
        // contain their own blink/animation layers as the artwork evolves.
        let mut table = Reader {
            b: &bytes[..end],
            at: HEADER + legacy_reference().palette().len() * 2,
        };
        for _ in 0..20 {
            table.u16().unwrap();
            table.img().unwrap();
            for _ in 0..GROUPS {
                let count = table.u16().unwrap();
                for _ in 0..count {
                    table.img().unwrap();
                }
            }
        }
        let legacy_end = table.at;
        let removed = end - legacy_end;
        bytes.drain(legacy_end..end);
        bytes[18..20].copy_from_slice(&20u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&((meta - removed) as u32).to_le_bytes());
        fix_crc(&mut bytes);
        let legacy = Skin::parse(bytes).unwrap();
        for pose in [
            FoxPose::NetworkConnected,
            FoxPose::BluetoothDisconnected,
            FoxPose::MusicPlaying,
        ] {
            assert_eq!(
                legacy.pose(pose).base.off,
                legacy.pose(pose.legacy_fallback()).base.off
            );
        }
        let plain = fixture(0);
        let mut compressed = Vec::from(*COMPRESSED_MAGIC);
        compressed.extend((plain.len() as u32).to_le_bytes());
        compressed.extend(miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9));
        assert!(compressed.len() < MAX_BYTES);
        assert_eq!(
            Skin::parse(compressed).unwrap().blob(),
            Skin::parse(plain).unwrap().blob()
        );
        let mut bomb = Vec::from(*COMPRESSED_MAGIC);
        bomb.extend((MAX_DECODED_BYTES as u32 + 1).to_le_bytes());
        assert!(Skin::parse(bomb).is_err());
    }
    #[test]
    fn external_default_is_pixel_identical_across_formats_and_recolor_invalidates_cache() {
        let default = Skin::parse(fixture(0)).unwrap();
        let recolor = Skin::parse(fixture(0x7800)).unwrap();
        let mut r = FoxRenderer::new();
        let mut a = vec![0; PIXELS];
        let mut b = vec![0; PIXELS];
        r.render(&Scene::with_skin(FoxPose::Idle, legacy_reference()), &mut a);
        r.render(&Scene::with_skin(FoxPose::Idle, default), &mut b);
        assert_eq!(a, b);
        let mut animator = FoxAnimator::new(FoxPose::Idle);
        animator.set_skin(Rc::clone(&recolor));
        r.render(&animator.scene(FoxInput::REST), &mut b);
        assert_ne!(a, b);
        r.render(&Scene::with_skin(FoxPose::Idle, recolor), &mut b);
        assert_ne!(a, b);
        r.render(&Scene::with_skin(FoxPose::Idle, legacy_reference()), &mut b);
        assert_eq!(a, b);
    }
    #[test]
    fn all_external_default_poses_and_active_patches_match_across_formats() {
        let skin = Skin::parse(fixture(0)).unwrap();
        let source = legacy_reference();
        let mut renderer = FoxRenderer::new();
        let mut a = vec![0; PIXELS];
        let mut b = vec![0; PIXELS];
        for pose in FoxPose::ALL {
            let mut builtin = FoxAnimator::with_skin(pose, Rc::clone(&source));
            let mut packed = FoxAnimator::with_skin(pose, Rc::clone(&skin));
            let mut blink_checked = false;
            // Advance the identical seeded schedules, rendering sparse fixed
            // mouth/spiral/symbol phases plus the first actual blink frame.
            for at in (10..=7000).step_by(10) {
                builtin.advance(10);
                packed.advance(10);
                let scene = builtin.scene(FoxInput::REST);
                let blink = scene
                    .under
                    .iter()
                    .any(|l| source.pose(pose).blink.iter().any(|i| i.off == l.img.off));
                if [10, 70, 130, 250, 700, 1200, 2300, 4000, 6500].contains(&at)
                    || (blink && !blink_checked)
                {
                    renderer.render(&scene, &mut a);
                    renderer.render(&packed.scene(FoxInput::REST), &mut b);
                    assert_eq!(a, b, "pose {pose:?} at {at}");
                    blink_checked |= blink;
                }
            }
            if !source.pose(pose).blink.is_empty() {
                assert!(blink_checked, "blink phase covered for {pose:?}");
            }
        }
        renderer.render(&Scene::still(FoxPose::Idle), &mut a);
        let before = renderer.reused;
        renderer.render(&Scene::still(FoxPose::Idle), &mut b);
        assert_eq!(
            renderer.reused,
            before + 1,
            "static placeholder scenes retain cache identity"
        );
    }

    #[test]
    fn rejects_bad_header_pose_dimensions_offsets_runs_palette_and_metadata() {
        let original = fixture(0);
        let colors = legacy_reference().palette().len();
        for (offset, bytes) in [
            (8, vec![2]),
            (12, vec![0, 0]),
            (18, vec![19, 0]),
            (HEADER + colors * 2, vec![1, 0]),
            (HEADER + colors * 2 + 2 + 4, vec![0, 0]),
            (HEADER + colors * 2 + 2 + 8, vec![255, 255, 255, 255]),
        ] {
            let mut bad = original.clone();
            bad[offset..offset + bytes.len()].copy_from_slice(&bytes);
            fix_crc(&mut bad);
            assert!(Skin::parse(bad).is_err(), "offset {offset}");
        }
        let meta = u32::from_le_bytes(original[20..24].try_into().unwrap()) as usize;
        for pair in [[0, 0], [255, 254], [255, 255]] {
            let mut bad = original.clone();
            bad[HEADER + meta..HEADER + meta + 2].copy_from_slice(&pair);
            fix_crc(&mut bad);
            assert!(Skin::parse(bad).is_err());
        }
        let mut bad = original.clone();
        bad.pop();
        assert!(Skin::parse(bad).is_err());
        let mut bad = original;
        bad[HEADER] ^= 1;
        assert!(Skin::parse(bad).is_err());
    }
    #[test]
    fn mutually_exclusive_spirals_share_capacity_but_simultaneous_mouth_and_blink_do_not() {
        let valid = fixture_with(0, |id, g| {
            if id == FoxPose::Confused as usize {
                for group in &mut g[8..11] {
                    let img = group[0];
                    group.resize(9, img);
                }
            }
        });
        assert!(
            Skin::parse(valid).is_ok(),
            "27 mutually exclusive spiral layers need only nine slots"
        );
        let invalid = fixture_with(0, |id, g| {
            if id == FoxPose::Speaking as usize {
                let blink = g[0][0];
                let mouth = g[1][0];
                g[0].resize(14, blink);
                g[1].resize(12, mouth);
            }
        });
        assert!(Skin::parse(invalid)
            .unwrap_err()
            .contains("simultaneous layer capacity"));
    }

    #[test]
    fn repeated_switches_release_resources_and_old_scenes_remain_renderable() {
        let mut animator = FoxAnimator::new(FoxPose::Happy);
        let mut renderer = FoxRenderer::new();
        let mut pixels = vec![0; PIXELS];
        let mut previous: Option<alloc::rc::Weak<Skin>> = None;
        for n in 0..20 {
            let skin = Skin::parse(fixture(n)).unwrap();
            let weak = Rc::downgrade(&skin);
            animator.set_skin(skin);
            let scene = animator.scene(FoxInput::REST);
            renderer.render(&scene, &mut pixels);
            if let Some(old) = previous.take() {
                assert!(old.upgrade().is_none());
            }
            previous = Some(weak);
        }
        let old_scene = animator.scene(FoxInput::REST);
        animator.set_skin(Skin::builtin());
        renderer.render(&old_scene, &mut pixels);
        let weak = previous.unwrap();
        assert!(weak.upgrade().is_some());
        drop(old_scene);
        renderer.render(&animator.scene(FoxInput::REST), &mut pixels);
        assert!(weak.upgrade().is_none());
    }
}
