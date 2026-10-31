//! Bounded A/B status reads. A valid descriptor is not proof of a complete image.
use alloc::{format, string::String};
use {
    crate::services::update::ota::slot_offset, crate::services::update::ota::Entry,
    crate::services::update::ota::Flash, crate::services::update::ota::SLOT_SIZE,
};

pub fn state_name(state: u32) -> &'static str {
    match state {
        0 => "New",
        1 => "PendingVerify",
        2 => "Valid",
        3 => "Invalid",
        4 => "Aborted",
        _ => "Unknown",
    }
}
fn text(bytes: &[u8]) -> Option<&str> {
    let end = bytes.iter().position(|&v| v == 0)?;
    if end == 0
        || !bytes[..end].iter().all(u8::is_ascii_graphic)
        || bytes[end..].iter().any(|&v| v != 0)
    {
        return None;
    }
    core::str::from_utf8(&bytes[..end]).ok()
}
fn version(prefix: &[u8; 288]) -> Option<&str> {
    let word = |at| u32::from_le_bytes(prefix[at..at + 4].try_into().unwrap());
    let address = word(24);
    let length = word(28);
    let bounded = address.checked_add(length).is_some_and(|end| {
        [
            (0x40000000, 0x54000000),
            (0x2f000000, 0x2f080000),
            (0x2e000000, 0x2e008000),
        ]
        .iter()
        .any(|&(lo, hi)| address >= lo && end <= hi)
    });
    if prefix[0] != 0xe9
        || !(1..=16).contains(&prefix[1])
        || prefix[2] != 2
        || prefix[3] != 0x40
        || u16::from_le_bytes([prefix[12], prefix[13]]) != 32
        || prefix[19..23] != [0; 4]
        || prefix[23] != 1
        || !(256..=SLOT_SIZE - 32).contains(&length)
        || length % 4 != 0
        || !bounded
        || word(32) != 0xabcd5432
        || text(&prefix[80..112]) != Some("mosaico-s31")
    {
        return None;
    }
    text(&prefix[48..80])
}
/// Reads two 32-byte metadata entries and one 288-byte prefix per slot.
/// Metadata state and descriptor validity are reported independently. No hash
/// or full-image validation is implied by `image=descriptor`.
pub fn slot_lines(flash: &mut impl Flash, booted: Option<u8>) -> [String; 2] {
    let mut entries = [None; 2];
    for (i, entry) in entries.iter_mut().enumerate() {
        let mut raw = [0; 32];
        if flash.read(0x9000 + i as u32 * 4096, &mut raw).is_ok() {
            *entry = Entry::parse(&raw).ok().flatten();
        }
    }
    core::array::from_fn(|slot| {
        let entry = entries
            .iter()
            .flatten()
            .filter(|e| e.slot() == slot as u8)
            .max_by_key(|e| e.seq);
        let state = entry.map_or("Unknown", |e| state_name(e.state));
        let seq = entry.map_or(0, |e| e.seq);
        let mut prefix = [0; 288];
        let (image, version) = if flash
            .read(slot_offset(slot as u8).unwrap(), &mut prefix)
            .is_err()
        {
            ("unknown", "unknown")
        } else if prefix.iter().all(|&v| v == 255) {
            ("empty", "empty")
        } else if let Some(version) = version(&prefix) {
            ("descriptor", version)
        } else {
            ("invalid", "invalid")
        };
        format!("@ota-slot slot={slot} booted={} state={state} seq={seq} image={image} version={version}",
            u8::from(booted == Some(slot as u8)))
    })
}
