use super::*;
use alloc::{vec, vec::Vec};
use md5::Md5;
struct Nor {
    bytes: Vec<u8>,
    writes: Vec<u32>,
    fail: Option<usize>,
    ops: usize,
}
impl Nor {
    fn new() -> Self {
        Self {
            bytes: vec![255; 16 * 1024 * 1024],
            writes: Vec::new(),
            fail: None,
            ops: 0,
        }
    }
    fn op(&mut self) -> Result<()> {
        self.ops += 1;
        if self.fail == Some(self.ops) {
            return Err(Error::Flash);
        }
        Ok(())
    }
}
impl Flash for Nor {
    fn read(&mut self, a: u32, out: &mut [u8]) -> Result<()> {
        self.op()?;
        out.copy_from_slice(&self.bytes[a as usize..a as usize + out.len()]);
        Ok(())
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<()> {
        self.op()?;
        self.writes.push(a);
        let dst = &mut self.bytes[a as usize..a as usize + data.len()];
        for (d, &s) in dst.iter_mut().zip(data) {
            assert_eq!(*d & s, s, "NOR overwrite");
            *d = s;
        }
        Ok(())
    }
    fn erase(&mut self, a: u32) -> Result<()> {
        self.op()?;
        assert_eq!(a % 4096, 0);
        self.writes.push(a);
        self.bytes[a as usize..a as usize + 4096].fill(255);
        Ok(())
    }
}
fn table(ab: bool) -> Vec<u8> {
    let mut b = Vec::new();
    let mut rows = vec![
        ("otadata", 1, 0, 0x9000u32, 0x2000u32),
        ("phy_init", 1, 1, 0xb000, 0x1000),
        ("sysmeta", 1, 2, 0xc000, 0x14000),
        ("vibe_mode", 0, 0x20, 0x20000, 0x1c0000),
        ("coredump", 1, 3, 0x1e0000, 0x20000),
        ("nvs", 1, 2, 0x200000, 0x6000),
        (
            "main_app",
            0,
            0x10,
            0x210000,
            if ab { 0x600000 } else { 0x7d0000 },
        ),
    ];
    if ab {
        rows.extend([
            ("ota_1", 0, 0x11, 0x810000, 0x600000),
            ("system", 1, 0x83, 0xe10000, 0x1f0000),
        ]);
    } else {
        rows.extend([
            ("ui_apps", 1, 0x41, 0x9e0000, 0x320000),
            ("system", 1, 0x83, 0xd00000, 0x300000),
        ]);
    }
    for (name, kind, subtype, offset, size) in rows {
        let mut e = [0; 32];
        e[..4].copy_from_slice(&[0xaa, 0x50, kind, subtype]);
        e[4..8].copy_from_slice(&offset.to_le_bytes());
        e[8..12].copy_from_slice(&size.to_le_bytes());
        e[12..12 + name.len()].copy_from_slice(name.as_bytes());
        b.extend(e);
    }
    let hash = Md5::digest(&b);
    b.extend([0xeb, 0xeb]);
    b.extend([255; 14]);
    b.extend_from_slice(&hash);
    b.resize(3072, 255);
    b
}
fn image(marker: bool) -> Vec<u8> {
    let mut b = vec![0; 24];
    b[0] = 0xe9;
    b[1] = 1;
    b[2] = 2;
    b[3] = 0x40;
    b[4..8].copy_from_slice(&0x40000100u32.to_le_bytes());
    b[12] = 32;
    b[23] = 1;
    b.extend(0x40000020u32.to_le_bytes());
    b.extend(512u32.to_le_bytes());
    let mut data = vec![0; 512];
    data[..4].copy_from_slice(&0xabcd5432u32.to_le_bytes());
    data[16..22].copy_from_slice(b"0.2.0\0");
    data[48..60].copy_from_slice(b"mosaico-s31\0");
    if marker {
        data[300..300 + AB_LAYOUT_MARKER.len()].copy_from_slice(AB_LAYOUT_MARKER);
    }
    let checksum = data.iter().fold(0xef, |s, &b| s ^ b);
    b.extend(data);
    while b.len() % 16 != 15 {
        b.push(0);
    }
    b.push(checksum);
    let hash = Sha256::digest(&b);
    b.extend_from_slice(&hash);
    b
}
#[test]
fn image_streaming_every_fragment_and_metadata() {
    let image = image(true);
    for chunk in [1, 7, 23, 256, 4096] {
        let mut verifier = image::ImageVerifier::new(image.len() as u32);
        for bytes in image.chunks(chunk) {
            verifier.feed(bytes).unwrap();
        }
        verifier.finish().unwrap();
        assert!(verifier.ab_layout());
    }
    let mut bad = image.clone();
    bad.push(1);
    let mut v = image::ImageVerifier::new(bad.len() as u32);
    assert!(v.feed(&bad).and_then(|_| v.finish()).is_err());
    assert!(parse_sha(&"é".repeat(32)).is_err());
}
#[test]
fn current_layout_only_with_data_on_nand() {
    assert!(layout::parse(&table(false), 16 << 20).is_err());
    let new = layout::parse(&table(true), 16 << 20).unwrap();
    assert_eq!(new.data, None);
    assert!(new.ab);
    let mut corrupted = table(true);
    corrupted[20] ^= 1;
    assert!(layout::parse(&corrupted, 16 << 20).is_err());
    assert!(layout::parse(&table(true), 4 << 20).is_err());
}
#[test]
fn ota_real_nor_activation_baseline_readback_and_confirmation() {
    let layout = layout::parse(&table(true), 16 << 20).unwrap();
    let mut flash = Nor::new();
    confirm(&mut flash, 0).unwrap();
    flash.writes.clear();
    let image = image(true);
    let mut updater = Updater::new(&layout, 0, &mut flash).unwrap();
    updater
        .begin(image.len() as u32, Sha256::digest(&image).into())
        .unwrap();
    updater
        .chunk(&mut flash, 0, &image, crate::formats::crc32::crc32(&image))
        .unwrap();
    assert!(flash.writes.iter().all(|&a| a >= 0x810000 && a < 0xe10000));
    updater.start_verify().unwrap();
    assert!(updater.verify_step(&mut flash).unwrap());
    let entries = read_entries(&mut flash).unwrap();
    assert_eq!(entries[0], Some(Entry { seq: 1, state: 2 }));
    assert_eq!(selected(entries).unwrap().unwrap().1.slot(), 1);
    confirm(&mut flash, 1).unwrap();
    assert_eq!(
        selected(read_entries(&mut flash).unwrap())
            .unwrap()
            .unwrap()
            .1
            .state,
        2
    );
    assert!(Updater::new(&layout, 0, &mut flash).is_err());
}
#[test]
fn corrupt_transport_image_and_missing_marker_never_activate() {
    for marker in [false, true] {
        let layout = layout::parse(&table(true), 16 << 20).unwrap();
        let mut flash = Nor::new();
        confirm(&mut flash, 0).unwrap();
        let baseline = flash.bytes[0x9000..0xb000].to_vec();
        let image = image(marker);
        let mut updater = Updater::new(&layout, 0, &mut flash).unwrap();
        updater
            .begin(image.len() as u32, Sha256::digest(&image).into())
            .unwrap();
        updater
            .chunk(&mut flash, 0, &image, crate::formats::crc32::crc32(&image))
            .unwrap();
        if marker {
            flash.bytes[0x810100] ^= 1;
        }
        updater.start_verify().unwrap();
        assert!(updater.verify_step(&mut flash).is_err());
        assert_eq!(flash.bytes[0x9000..0xb000], baseline);
    }
}
#[test]
fn torn_alternate_entry_preserves_current_and_recovers() {
    assert_eq!(entry_crc(1), 0x4743989a);
    let mut flash = Nor::new();
    persist_entry(&mut flash, 0, Entry { seq: 1, state: 2 }).unwrap();
    flash.bytes[0xa000..0xa003].copy_from_slice(&[2, 0, 0]);
    assert_eq!(
        selected(read_entries(&mut flash).unwrap())
            .unwrap()
            .unwrap()
            .1
            .slot(),
        0
    );
    select_slot(&mut flash, 0, 1, 0).unwrap();
    assert_eq!(
        selected(read_entries(&mut flash).unwrap())
            .unwrap()
            .unwrap()
            .1
            .slot(),
        1
    );
}
#[test]
fn binary_parser_discards_bad_header_body_and_poison_requires_reconnect() {
    use {crate::console::protocol::Effect, crate::console::protocol::Session};
    let mut session = Session::new();
    let mut vfs = ::fs::Vfs::new();
    let body = b"@mosaico reboot\n";
    let mut wire = format!("@mosaico ota-chunk bad {} zzzzzzzz\n", body.len()).into_bytes();
    wire.extend(body);
    let mut effects = Vec::new();
    for byte in wire {
        session.feed(&mut vfs, &[byte], &mut |e| effects.push(e));
    }
    assert!(effects
        .iter()
        .all(|e| matches!(e, Effect::Reply(_) | Effect::OtaCancel)));
    effects.clear();
    session.feed(
        &mut vfs,
        b"@mosaico ota-chunk 0 4097 00000000\n@mosaico reboot\n",
        &mut |e| effects.push(e),
    );
    assert!(effects
        .iter()
        .all(|e| matches!(e, Effect::Reply(_) | Effect::OtaCancel)));
}

#[test]
fn unconfirmed_boot_never_overwrites_rollback_slot() {
    let layout = layout::parse(&table(true), 16 << 20).unwrap();
    let mut flash = Nor::new();
    let mut up = Updater::new(&layout, 0, &mut flash).unwrap();
    assert!(up.begin(1000, [0; 32]).is_err());
    assert!(flash.writes.is_empty());
    confirm(&mut flash, 0).unwrap();
    select_slot(&mut flash, 0, 1, 0).unwrap();
    let mut up = Updater::new(&layout, 1, &mut flash).unwrap();
    assert!(up.begin(1000, [0; 32]).is_err());
    confirm(&mut flash, 1).unwrap();
    assert!(Updater::new(&layout, 1, &mut flash)
        .unwrap()
        .begin(1000, [0; 32])
        .is_ok());
}
#[test]
fn metadata_activation_fault_at_every_io_preserves_valid_old_copy() {
    for cut in 1..=5 {
        let mut flash = Nor::new();
        confirm(&mut flash, 0).unwrap();
        let baseline = flash.bytes[0x9000..0xa000].to_vec();
        flash.ops = 0;
        flash.fail = Some(cut);
        assert!(select_slot(&mut flash, 0, 1, 0).is_err());
        flash.fail = None;
        assert_eq!(&flash.bytes[0x9000..0xa000], baseline.as_slice());
        let entries = read_entries(&mut flash).unwrap();
        assert_eq!(entries[0], Some(Entry { seq: 1, state: 2 }));
        // A failed readback may still have fully persisted the new selection.
        let chosen = selected(entries).unwrap().unwrap().1;
        assert!(chosen.slot() == 0 || chosen.slot() == 1);
    }
}
#[test]
fn noalloc_layout_agrees_on_valid_and_rejects_mutations() {
    let valid = table(true);
    assert!(layout::verify_ab_early(&valid, 16 << 20).is_ok());
    assert!(layout::verify_ab_early(&table(false), 16 << 20).is_err());
    for (at, value) in [
        (28, 1),
        (5, 0),
        (3, 0x11),
        (12, b'x'),
        (100, 0xff),
        (300, 0),
    ] {
        let mut bad = valid.clone();
        bad[at] = value;
        // Repair the digest to exercise semantic checks when the mutation hits a record.
        let md5_at = 7 * 32;
        let digest = Md5::digest(&bad[..md5_at]);
        bad[md5_at + 16..md5_at + 32].copy_from_slice(&digest);
        assert!(
            layout::verify_ab_early(&bad, 16 << 20).is_err(),
            "mutation {at}"
        );
    }
}

#[test]
fn selftest_requires_continuous_heartbeats_and_resets_on_fault() {
    let mut window = HealthWindow::new();
    for n in 0..30 {
        assert!(!window.observe(n * 1000, true));
    }
    assert!(window.observe(30_000, true));
    assert!(!window.observe(59_000, true)); // long stall resets probation
    for n in 60..89 {
        assert!(!window.observe(n * 1000, true));
    }
    assert!(window.observe(89_000, true));
    assert!(!window.observe(89_001, false));
    assert!(!window.observe(89_002, true));
    assert!(!window.observe(10, true)); // clock regression also resets
}

#[test]
fn status_reads_bounded_descriptors_and_reports_both_slots() {
    let mut flash = Nor::new();
    let lines = status::slot_lines(&mut flash, Some(0));
    assert!(lines[0].contains("state=Unknown") && lines[0].contains("image=empty"));
    assert!(lines[1].contains("booted=0"));
    let valid = image(true);
    flash.bytes[0x210000..0x210000 + valid.len()].copy_from_slice(&valid);
    confirm(&mut flash, 0).unwrap();
    let lines = status::slot_lines(&mut flash, Some(0));
    assert!(lines[0].contains("state=Valid") && lines[0].contains("version=0.2.0"));
    for (at, value) in [(0, 0), (12, 9), (29, 0), (32, 0), (48, 0), (80, b'x')] {
        let old = flash.bytes[0x210000 + at];
        flash.bytes[0x210000 + at] = value;
        assert!(
            status::slot_lines(&mut flash, Some(0))[0].contains("image=invalid"),
            "offset {at}"
        );
        flash.bytes[0x210000 + at] = old;
    }
    for (state, name) in [
        (0, "New"),
        (1, "PendingVerify"),
        (2, "Valid"),
        (3, "Invalid"),
        (4, "Aborted"),
        (u32::MAX, "Unknown"),
    ] {
        let raw = Entry { seq: 2, state }.bytes();
        flash.bytes[0xa000..0xa020].copy_from_slice(&raw);
        let lines = status::slot_lines(&mut flash, Some(0));
        assert!(lines[1].contains(&format!("state={name}")));
        assert!(lines.iter().all(|line| line.len() <= 158));
    }
    flash.fail = Some(flash.ops + 3); // slot-zero prefix read fails
    assert!(status::slot_lines(&mut flash, None)[0].contains("image=unknown"));
}

#[test]
fn warm_fallback_and_ack_reset_policy_have_explicit_bounds() {
    assert!(!warm_fallback_due(0));
    assert!(!warm_fallback_due(1));
    assert!(!warm_fallback_due(2));
    assert!(warm_fallback_due(3));
    assert!(warm_fallback_due(u32::MAX));
    assert_eq!(reboot_ack(false, 0), RebootAck::Waiting);
    assert_eq!(reboot_ack(false, 2999), RebootAck::Waiting);
    assert_eq!(reboot_ack(false, 3000), RebootAck::TimedOut);
    assert_eq!(reboot_ack(false, u64::MAX), RebootAck::TimedOut);
    assert_eq!(reboot_ack(true, 0), RebootAck::Sent);
    assert_eq!(reboot_ack(true, 3000), RebootAck::Sent);
}

#[test]
fn oversized_and_non_utf8_headers_poison_raw_body_until_reset() {
    use {crate::console::protocol::Effect, crate::console::protocol::Session};
    let mut too_long = b"@mosaico ota-chunk ".to_vec();
    too_long.extend(core::iter::repeat_n(b'9', 300));
    too_long.push(b'\n');
    let bad_utf8 = b"@mosaico ota-chunk 0 40 00000000\xff\n".to_vec();
    for header in [too_long, bad_utf8] {
        for fragmented in [false, true] {
            let mut session = Session::new();
            session.set_ota_active(true);
            let mut vfs = ::fs::Vfs::new();
            let mut wire = header.clone();
            wire.extend_from_slice(b"@mosaico reboot\n@mosaico ota-end\n@mosaico hello\n");
            let mut effects = Vec::new();
            let width = if fragmented { 1 } else { wire.len() };
            for chunk in wire.chunks(width) {
                session.feed(&mut vfs, chunk, &mut |e| effects.push(e));
            }
            assert_eq!(effects.len(), 2);
            assert!(matches!(effects[0], Effect::OtaCancel));
            assert!(
                matches!(&effects[1],Effect::Reply(text) if text.starts_with("@err") && text.contains("reconnect required"))
            );
            // Even a later complete command cannot escape the poisoned framing.
            session.feed(&mut vfs, b"@mosaico reboot\n@mosaico ota-end\n", &mut |e| {
                effects.push(e)
            });
            assert_eq!(effects.len(), 2);
            session.reset(&mut vfs);
            effects.clear();
            session.feed(&mut vfs, b"@mosaico hello\n", &mut |e| effects.push(e));
            assert!(effects
                .iter()
                .any(|e| matches!(e,Effect::Reply(text) if text.starts_with("@ok"))));
        }
    }
}
