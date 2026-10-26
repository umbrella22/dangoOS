//! Strict partition-table digest and layout checks before any writable region.
use super::{Error, Result, SLOT_SIZE};
use alloc::{string::String, vec::Vec};
use md5::{Digest, Md5};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub label: String,
    pub kind: u8,
    pub subtype: u8,
    pub offset: u32,
    pub size: u32,
    pub flags: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub entries: Vec<Partition>,
    pub ab: bool,
    pub data: Option<(u32, u32)>,
}
pub fn parse(bytes: &[u8], capacity: u32) -> Result<Layout> {
    let mut entries: Vec<Partition> = Vec::new();
    let mut digest = false;
    let mut end = 0;
    for (i, e) in bytes.chunks_exact(32).enumerate() {
        if e[..2] == [0xeb, 0xeb] {
            if entries.is_empty()
                || e[2..16] != [0xff; 14]
                || Md5::digest(&bytes[..i * 32])[..] != e[16..32]
            {
                return Err(Error::Layout("partition MD5"));
            }
            digest = true;
            end = (i + 1) * 32;
            break;
        }
        if e[..2] != [0xaa, 0x50] {
            return Err(Error::Layout("partition entry/missing MD5"));
        }
        let word = |n| u32::from_le_bytes(e[n..n + 4].try_into().unwrap());
        let offset = word(4);
        let size = word(8);
        let flags = word(28);
        let n = e[12..28].iter().position(|&b| b == 0).unwrap_or(16);
        let label =
            core::str::from_utf8(&e[12..12 + n]).map_err(|_| Error::Layout("partition label"))?;
        if label.is_empty()
            || flags != 0
            || size == 0
            || offset % 4096 != 0
            || size % 4096 != 0
            || offset < 0x9000
            || offset.checked_add(size).is_none_or(|v| v > capacity)
            || entries.iter().any(|p| {
                p.label == label || (offset < p.offset + p.size && p.offset < offset + size)
            })
        {
            return Err(Error::Layout("partition flags/range/overlap/duplicate"));
        }
        if e[2] == 0 && offset % 65536 != 0 {
            return Err(Error::Layout("unaligned app partition"));
        }
        // otadata lives below the minimum regular-partition bound.
        entries.push(Partition {
            label: label.into(),
            kind: e[2],
            subtype: e[3],
            offset,
            size,
            flags,
        });
    }
    if !digest || bytes[end..].iter().any(|&v| v != 0xff) {
        return Err(Error::Layout("partition digest/trailer"));
    }
    inspect(entries)
}
fn inspect(entries: Vec<Partition>) -> Result<Layout> {
    let find = |name: &str| entries.iter().find(|p| p.label == name);
    let main = find("main_app").ok_or(Error::Layout("main_app missing"))?;
    let ab =
        main.kind == 0 && main.subtype == 0x10 && main.offset == 0x210000 && main.size == SLOT_SIZE;
    if entries.iter().any(|p| {
        p.kind == 0
            && (p.subtype == 0 || (0x10..=0x1f).contains(&p.subtype))
            && !(p.label == "main_app" && p.subtype == 0x10
                || ab && p.label == "ota_1" && p.subtype == 0x11)
    }) {
        return Err(Error::Layout("extra factory/OTA application"));
    }
    let data = if ab {
        for (label, kind, subtype, offset, size) in [
            ("main_app", 0, 0x10, 0x210000, SLOT_SIZE),
            ("ota_1", 0, 0x11, 0x510000, SLOT_SIZE),
            ("data", 1, 0x83, 0x810000, 0x1d0000),
            ("otadata", 1, 0, 0x9000, 0x2000),
        ] {
            let p = find(label).ok_or(Error::Layout("A/B partition missing"))?;
            if (p.kind, p.subtype, p.offset, p.size) != (kind, subtype, offset, size) {
                return Err(Error::Layout("A/B layout mismatch"));
            }
        }
        Some((0x810000, 0x1d0000))
    } else if main.kind == 0
        && main.subtype == 0x10
        && main.offset == 0x210000
        && main.size == 0x7d0000
        && !entries.iter().any(|p| p.subtype == 0x11 && p.kind == 0)
        && find("data").is_none()
    {
        Some((0x7e0000, 0x200000))
    } else {
        return Err(Error::Layout("unknown app/data layout"));
    };
    for (label, kind, subtype, offset) in [
        ("vibe_mode", 0, 0x20, 0x20000),
        ("ui_apps", 1, 0x41, 0x9e0000),
        ("system", 1, 0x83, 0xd00000),
    ] {
        if find(label).is_none_or(|p| (p.kind, p.subtype, p.offset) != (kind, subtype, offset)) {
            return Err(Error::Layout("protected partition missing/moved"));
        }
    }
    Ok(Layout { entries, ab, data })
}

/// Validate the complete A/B table without invoking the heap. Used before PSRAM init.
pub fn verify_ab_early(bytes: &[u8], capacity: u32) -> Result<()> {
    let mut count = 0usize;
    let mut found = 0u8;
    let mut digest_end = None;
    for (i, e) in bytes.chunks_exact(32).enumerate() {
        if e[..2] == [0xeb, 0xeb] {
            if count == 0
                || e[2..16] != [0xff; 14]
                || Md5::digest(&bytes[..i * 32])[..] != e[16..32]
            {
                return Err(Error::Layout("partition MD5"));
            }
            digest_end = Some((i + 1) * 32);
            break;
        }
        if e[..2] != [0xaa, 0x50] {
            return Err(Error::Layout("partition entry"));
        }
        let word = |n| u32::from_le_bytes(e[n..n + 4].try_into().unwrap());
        let offset = word(4);
        let size = word(8);
        let end = offset
            .checked_add(size)
            .ok_or(Error::Layout("partition overflow"))?;
        let n = e[12..28].iter().position(|&b| b == 0).unwrap_or(16);
        let label =
            core::str::from_utf8(&e[12..12 + n]).map_err(|_| Error::Layout("partition label"))?;
        if label.is_empty()
            || word(28) != 0
            || size == 0
            || offset < 0x9000
            || offset % 4096 != 0
            || size % 4096 != 0
            || end > capacity
            || e[2] == 0 && offset % 65536 != 0
        {
            return Err(Error::Layout("partition bounds"));
        }
        for p in bytes[..i * 32].chunks_exact(32) {
            let po = u32::from_le_bytes(p[4..8].try_into().unwrap());
            let ps = u32::from_le_bytes(p[8..12].try_into().unwrap());
            let pn = p[12..28].iter().position(|&b| b == 0).unwrap_or(16);
            if p[12..12 + pn] == e[12..12 + n] || offset < po + ps && po < end {
                return Err(Error::Layout("partition overlap/duplicate"));
            }
        }
        let expected = match label {
            "main_app" => Some((0, 0x10, 0x210000, Some(SLOT_SIZE), 1)),
            "ota_1" => Some((0, 0x11, 0x510000, Some(SLOT_SIZE), 2)),
            "data" => Some((1, 0x83, 0x810000, Some(0x1d0000), 4)),
            "otadata" => Some((1, 0, 0x9000, Some(0x2000), 8)),
            "vibe_mode" => Some((0, 0x20, 0x20000, None, 16)),
            "ui_apps" => Some((1, 0x41, 0x9e0000, None, 32)),
            "system" => Some((1, 0x83, 0xd00000, None, 64)),
            _ => None,
        };
        if let Some((kind, subtype, at, length, bit)) = expected {
            if (e[2], e[3], offset) != (kind, subtype, at) || length.is_some_and(|v| v != size) {
                return Err(Error::Layout("A/B layout mismatch"));
            }
            found |= bit;
        } else if e[2] == 0 && (e[3] == 0 || (0x10..=0x1f).contains(&e[3])) {
            return Err(Error::Layout("extra factory/OTA application"));
        }
        count += 1;
    }
    let end = digest_end.ok_or(Error::Layout("partition digest missing"))?;
    if found != 127 || bytes[end..].iter().any(|&v| v != 0xff) {
        return Err(Error::Layout("A/B partitions/trailer"));
    }
    Ok(())
}
