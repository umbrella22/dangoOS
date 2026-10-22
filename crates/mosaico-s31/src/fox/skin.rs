//! Validated, owned skin resources. No file-backed pointer reaches the renderer.
use super::{data, FoxPose, Img, Pose, H, W};
use alloc::{rc::Rc, string::String, vec::Vec};

pub const MAGIC: &[u8; 8] = b"MSKIN01\0";
pub const HEADER: usize = 36;
pub const MAX_BYTES: usize = 1024 * 1024;
pub const POSES: usize = 20;
pub const GROUPS: usize = 11;

#[derive(Debug)]
pub struct OwnedPose {
    pub base: Img,
    pub groups: [Vec<Img>; GROUPS],
}
/// Borrowed view shared by the compiled default and validated packages.
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
    blob: Vec<u8>,
}
#[derive(Debug)]
pub struct Skin {
    package: Option<Package>,
}
impl Skin {
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
        self.package.as_ref().map_or(data::BLOB, |p| &p.blob)
    }
    pub fn pose(&self, pose: FoxPose) -> PoseView<'_> {
        let Some(package) = &self.package else {
            return pose.data().into();
        };
        let p = &package.poses[pose as usize];
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
        if bytes.len() < HEADER || bytes.len() > MAX_BYTES || &bytes[..8] != MAGIC {
            return Err("skin binary magic/size".into());
        }
        let mut r = Reader { b: &bytes, at: 8 };
        if r.u32()? != 1 || r.u16()? as usize != W || r.u16()? as usize != H {
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
            || poses != POSES
            || reserved != 0
            || meta > 65536
            || HEADER
                .checked_add(meta)
                .and_then(|n| n.checked_add(blob_len))
                != Some(bytes.len())
        {
            return Err("skin binary table/length".into());
        }
        if crate::hub::push::crc32(&bytes[HEADER..]) != crc {
            return Err("skin binary checksum".into());
        }
        let end = HEADER + meta;
        r.b = &bytes[..end];
        let palette = (0..colors)
            .map(|_| r.u16())
            .collect::<Result<Vec<_>, _>>()?;
        let mut table = Vec::with_capacity(POSES);
        for id in 0..POSES {
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
            validate_img(&base, &bytes[end..], colors, true)?;
            for img in groups.iter().flatten() {
                validate_img(img, &bytes[end..], colors, false)?;
            }
            table.push(OwnedPose { base, groups });
        }
        if r.at != end {
            return Err("skin trailing metadata".into());
        }
        // All metadata and RLE have been checked before the resource can exist.
        let blob = bytes[end..].to_vec();
        Ok(Rc::new(Self {
            package: Some(Package {
                palette,
                poses: table,
                blob,
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
    if u > super::scene::MAX_LAYERS || o > super::scene::MAX_LAYERS {
        return Err("skin simultaneous layer capacity".into());
    }
    Ok(())
}

fn validate_img(img: &Img, blob: &[u8], colors: usize, opaque: bool) -> Result<(), String> {
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
impl Reader<'_> {
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

#[cfg(test)]
pub(crate) fn fixture(xor: u16) -> Vec<u8> {
    fixture_with(xor, |_, _| {})
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
    let mut meta = Vec::new();
    for c in data::PALETTE {
        meta.extend((c ^ xor).to_le_bytes());
    }
    for (id, p) in data::POSES.iter().enumerate() {
        meta.extend((id as u16).to_le_bytes());
        img(&mut meta, &p.base);
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
        .map(|g| g.to_vec());
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
    for n in [W as u16, H as u16, data::PALETTE.len() as u16, POSES as u16] {
        out.extend(n.to_le_bytes());
    }
    for n in [meta.len() as u32, data::BLOB.len() as u32, 0, 0] {
        out.extend(n.to_le_bytes());
    }
    out.extend(meta);
    out.extend_from_slice(data::BLOB);
    fix_crc(&mut out);
    out
}
#[cfg(test)]
pub(crate) fn fix_crc(b: &mut [u8]) {
    let crc = crate::hub::push::crc32(&b[HEADER..]);
    b[28..32].copy_from_slice(&crc.to_le_bytes());
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fox::{FoxAnimator, FoxInput, FoxRenderer, Scene, PIXELS};
    use alloc::vec;
    #[test]
    fn packed_default_is_pixel_identical_and_recolor_changes_pixels_with_cache_invalidation() {
        let default = Skin::parse(fixture(0)).unwrap();
        let recolor = Skin::parse(fixture(0x7800)).unwrap();
        let mut r = FoxRenderer::new();
        let mut a = vec![0; PIXELS];
        let mut b = vec![0; PIXELS];
        r.render(&Scene::still(FoxPose::Idle), &mut a);
        r.render(&Scene::with_skin(FoxPose::Idle, default), &mut b);
        assert_eq!(a, b);
        let mut animator = FoxAnimator::new(FoxPose::Idle);
        animator.set_skin(Rc::clone(&recolor));
        r.render(&animator.scene(FoxInput::REST), &mut b);
        assert_ne!(a, b);
        r.render(&Scene::with_skin(FoxPose::Idle, recolor), &mut b);
        assert_ne!(a, b);
        r.render(&Scene::still(FoxPose::Idle), &mut b);
        assert_eq!(a, b);
    }
    #[test]
    fn all_packed_default_poses_and_active_patches_match_builtin_rendering() {
        let skin = Skin::parse(fixture(0)).unwrap();
        let mut renderer = FoxRenderer::new();
        let mut a = vec![0; PIXELS];
        let mut b = vec![0; PIXELS];
        for pose in FoxPose::ALL {
            let mut builtin = FoxAnimator::new(pose);
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
                    .any(|l| pose.data().blink.iter().any(|i| i.off == l.img.off));
                if [10, 70, 130, 250, 700, 1200, 2300, 4000, 6500].contains(&at)
                    || (blink && !blink_checked)
                {
                    renderer.render(&scene, &mut a);
                    renderer.render(&packed.scene(FoxInput::REST), &mut b);
                    assert_eq!(a, b, "pose {pose:?} at {at}");
                    blink_checked |= blink;
                }
            }
            if !pose.data().blink.is_empty() {
                assert!(blink_checked, "blink phase covered for {pose:?}");
            }
        }
        renderer.render(&Scene::still(FoxPose::Idle), &mut a);
        let before = renderer.reused;
        renderer.render(&Scene::still(FoxPose::Idle), &mut b);
        assert_eq!(
            renderer.reused,
            before + 1,
            "compiled-default scenes retain cache identity"
        );
    }

    #[test]
    fn rejects_bad_header_pose_dimensions_offsets_runs_palette_and_metadata() {
        let original = fixture(0);
        for (offset, bytes) in [
            (8, vec![2]),
            (12, vec![0, 0]),
            (18, vec![19, 0]),
            (HEADER + data::PALETTE.len() * 2, vec![1, 0]),
            (HEADER + data::PALETTE.len() * 2 + 2 + 4, vec![0, 0]),
            (
                HEADER + data::PALETTE.len() * 2 + 2 + 8,
                vec![255, 255, 255, 255],
            ),
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
