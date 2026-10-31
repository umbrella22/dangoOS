//! Strict partition-table digest and layout checks before any writable region.
use alloc::{string::String, vec::Vec};
use md5::{Digest, Md5};
use {
    crate::services::update::ota::Error, crate::services::update::ota::Result,
    crate::services::update::ota::SLOT_SIZE,
};
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
/// Exact current NOR contract. Writable application data lives on NAND.
pub const EXPECTED: [(&str, u8, u8, u32, u32); 9] = [
    ("otadata", 1, 0, 0x9000, 0x2000),
    ("phy_init", 1, 1, 0xb000, 0x1000),
    ("sysmeta", 1, 2, 0xc000, 0x14000),
    ("vibe_mode", 0, 0x20, 0x20000, 0x1c0000),
    ("coredump", 1, 3, 0x1e0000, 0x20000),
    ("nvs", 1, 2, 0x200000, 0x6000),
    ("main_app", 0, 0x10, 0x210000, SLOT_SIZE),
    ("ota_1", 0, 0x11, 0x810000, SLOT_SIZE),
    ("system", 1, 0x83, 0xe10000, 0x1f0000),
];
fn inspect(entries: Vec<Partition>) -> Result<Layout> {
    if entries.len() != EXPECTED.len() {
        return Err(Error::Layout("partition count"));
    }
    for &(label, kind, subtype, offset, size) in &EXPECTED {
        let p = entries
            .iter()
            .find(|p| p.label == label)
            .ok_or(Error::Layout("A/B partition missing"))?;
        if (p.kind, p.subtype, p.offset, p.size) != (kind, subtype, offset, size) {
            return Err(Error::Layout("A/B layout mismatch"));
        }
    }
    Ok(Layout {
        entries,
        ab: true,
        data: None,
    })
}

/// Validate the complete A/B table without invoking the heap. Used before PSRAM init.
pub fn verify_ab_early(bytes: &[u8], capacity: u32) -> Result<()> {
    let mut count = 0usize;
    let mut found = 0u16;
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
        let (index, &(_, kind, subtype, at, length)) = EXPECTED
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.0 == label)
            .ok_or(Error::Layout("unknown partition"))?;
        if (e[2], e[3], offset, size) != (kind, subtype, at, length) {
            return Err(Error::Layout("A/B layout mismatch"));
        }
        found |= 1 << index;
        count += 1;
    }
    let end = digest_end.ok_or(Error::Layout("partition digest missing"))?;
    if count != EXPECTED.len()
        || found != (1 << EXPECTED.len()) - 1
        || bytes[end..].iter().any(|&v| v != 0xff)
    {
        return Err(Error::Layout("A/B partitions/trailer"));
    }
    Ok(())
}
