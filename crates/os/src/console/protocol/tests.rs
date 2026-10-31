//! Protocol regression cases, including packet splits and deferred boundaries.
use super::*;
use crate::formats::crc32::crc32;
use ::fs::{Fs, Geometry, RamDevice};
use alloc::{format, string::String, vec::Vec};
use {crate::apps::registry::IconCache, crate::apps::registry::Registry};

fn vfs() -> Vfs {
    let mut v = Vfs::new();
    for (name, blocks) in [("system", 32), ("data", 64)] {
        let fs = Fs::mount_or_format(
            Box::new(RamDevice::new(4096, blocks)),
            Geometry::flash(blocks),
        )
        .unwrap()
        .0;
        v.mount(name, fs);
    }
    v
}

fn file_cmd(rel: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("@mosaico file {rel} {} {:08x}\n", data.len(), crc32(data)).into_bytes();
    out.extend_from_slice(data);
    out
}

const MANIFEST_TEXT: &str =
    r#"{"schema_version":1,"id":"hello","entry":"scripts/main.lua","display_name":"Hi"}"#;
const MAIN: &[u8] = b"local x = 1\nreturn x\n";

fn push_stream(run: bool) -> Vec<u8> {
    let mut s = b"noise from a terminal\n@mosaico hello\r\n@mosaico begin hello\n".to_vec();
    s.extend(file_cmd("launcher.json", MANIFEST_TEXT.as_bytes()));
    s.extend(file_cmd("scripts/main.lua", MAIN));
    s.extend(file_cmd("empty.txt", b""));
    s.extend_from_slice(if run {
        b"@mosaico commit hello run\n"
    } else {
        b"@mosaico commit hello\n"
    });
    s
}

fn run(v: &mut Vfs, sess: &mut Session, data: &[u8], chunk: usize) -> Vec<Effect> {
    let mut effects = Vec::new();
    for c in data.chunks(chunk) {
        sess.feed(v, c, &mut |e| effects.push(e));
    }
    effects
}

fn replies(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Reply(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn full_update_host_survives_console_framing_and_usb_packet_splits() {
    let host = format!(
        "{}.{}.{}.{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    assert_eq!(host.len(), 253);
    let url = format!("https://{host}/release.json");
    let packet = format!("@mosaico update-url {url}\n@mosaico hello\n");
    for chunk in [1, 64, 512] {
        let effects = run(
            &mut Vfs::new(),
            &mut Session::new(),
            packet.as_bytes(),
            chunk,
        );
        assert_eq!(
            effects,
            [
                Effect::Update(crate::services::update::Command::InstallUrl(
                    crate::services::update::Url::parse(&url).unwrap()
                )),
                Effect::Reply("@ok hello mosaico-push 1".into()),
            ]
        );
    }
}

#[test]
fn hardware_commands_are_deferred_at_the_exact_packet_boundary() {
    let mut v = Vfs::new(); // Discovery must also work without mounted storage.
    for (command, expected) in [
        (
            "update-status",
            Effect::Update(crate::services::update::Command::Status),
        ),
        (
            "update-check",
            Effect::Update(crate::services::update::Command::Check),
        ),
        (
            "update-install",
            Effect::Update(crate::services::update::Command::Install),
        ),
        (
            "update-cancel",
            Effect::Update(crate::services::update::Command::Cancel),
        ),
        (
            "wifi-status",
            Effect::Radio(crate::services::radio::Command::WifiStatus),
        ),
        (
            "wifi-scan",
            Effect::Radio(crate::services::radio::Command::WifiScan),
        ),
        (
            "wifi-on",
            Effect::Radio(crate::services::radio::Command::WifiOn),
        ),
        (
            "bt-on",
            Effect::Radio(crate::services::radio::Command::BtOn),
        ),
        (
            "bt-off",
            Effect::Radio(crate::services::radio::Command::BtOff),
        ),
        ("nand-info", Effect::NandInfo),
        ("nand-status", Effect::NandStatus),
        ("nand-format", Effect::NandFormat),
        (
            "screen-off 30",
            Effect::Power(crate::services::power::standby::Command::ScreenOff(30)),
        ),
        (
            "screen-on",
            Effect::Power(crate::services::power::standby::Command::ScreenOn),
        ),
        (
            "power-status",
            Effect::Power(crate::services::power::standby::Command::Status),
        ),
    ] {
        let first = format!("@mosaico {command}\n");
        let packet = format!("{first}@mosaico hello\n");
        let mut session = Session::new();
        let mut effects = Vec::new();
        let n = session.feed_until_deferred(&mut v, packet.as_bytes(), &mut |e| effects.push(e));
        assert_eq!(n, first.len());
        assert_eq!(effects, [expected]);
        session.feed_until_deferred(&mut v, &packet.as_bytes()[n..], &mut |e| effects.push(e));
        assert_eq!(effects[1], Effect::Reply("@ok hello mosaico-push 1".into()));
    }
}

#[test]
fn malformed_or_busy_hardware_commands_never_reach_devices() {
    let mut v = Vfs::new();
    for command in [
        "nand-info extra",
        "nand-read",
        "nand-read -1",
        "nand-read +1",
        "nand-read 4294967296",
        "nand-read 3 extra",
        "nand-format extra",
        "nand-test erase",
        "screen-off",
        "screen-off 0",
        "screen-off 61",
        "screen-on extra",
        "imu-irq both",
        "imu-irq int1 extra",
        "wifi-connect 61 0102030405060708",
        "wifi-status extra",
        "bt-on extra",
        "wifi-probe localhost 0",
    ] {
        let effects = run(
            &mut v,
            &mut Session::new(),
            format!("@mosaico {command}\n").as_bytes(),
            1,
        );
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0],Effect::Reply(s) if s.starts_with("@err")));
    }
    for busy in 0..3 {
        for command in [
            "nand-info",
            "nand-read 0",
            "nand-format",
            "nand-test write",
            "screen-off 30",
            "imu-irq int1",
            "wifi-scan",
            "bt-on",
        ] {
            let mut session = Session::new();
            match busy {
                0 => session.set_ota_active(true),
                1 => session.set_management_busy(true),
                _ => session.staging = Some("app".into()),
            }
            let effects = run(
                &mut v,
                &mut session,
                format!("@mosaico {command}\n").as_bytes(),
                64,
            );
            assert_eq!(effects.len(), 1);
            assert!(matches!(&effects[0],Effect::Reply(s) if s.starts_with("@err")));
        }
    }
}

#[test]
fn ota_allows_passive_power_status_without_unlocking_mutations() {
    let mut v = Vfs::new();
    let mut session = Session::new();
    session.set_ota_active(true);
    let e = run(
            &mut v,
            &mut session,
            b"@mosaico power-status\n@mosaico screen-off 30\n@mosaico wifi-status\n@mosaico bt-status\n@mosaico wifi-scan\n",
            512,
        );
    assert_eq!(
        e,
        [
            Effect::Power(crate::services::power::standby::Command::Status),
            Effect::Reply("@err OTA busy".into()),
            Effect::Radio(crate::services::radio::Command::WifiStatus),
            Effect::Radio(crate::services::radio::Command::BtStatus),
            Effect::Reply("@err OTA busy".into()),
        ]
    );
    assert!(session.ota_active);
}

#[test]
fn photo_delivery_atomically_replaces_only_the_selected_album_file() {
    for chunk in [1, 7, 64, 1024] {
        let mut v = vfs();
        v.create_dir_all(PHOTO_ROOT).unwrap();
        v.write_file(&format!("{PHOTO_ROOT}/first.jpg"), b"old")
            .unwrap();
        v.write_file(&format!("{PHOTO_ROOT}/keep.bin"), b"keep")
            .unwrap();
        v.write_file("/data/legacy.jpg", b"legacy").unwrap();
        let mut s = Session::new();
        let mut input = b"@mosaico photo-begin\n".to_vec();
        input.extend(file_cmd("first.jpg", b"replacement\0pixels"));
        let r = replies(&run(&mut v, &mut s, &input, chunk));
        assert_eq!(r.last().unwrap(), "@ok file first.jpg");
        assert_eq!(
            v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
            b"old"
        );
        let r = replies(&run(
            &mut v,
            &mut s,
            b"@mosaico photo-commit first.jpg\n",
            chunk,
        ));
        assert_eq!(r, ["@ok photo-commit first.jpg"]);
        assert_eq!(
            v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
            b"replacement\0pixels"
        );
        assert_eq!(
            v.read_file(&format!("{PHOTO_ROOT}/keep.bin")).unwrap(),
            b"keep"
        );
        assert_eq!(v.read_file("/data/legacy.jpg").unwrap(), b"legacy");
        assert!(v.stat(PHOTO_STAGING).is_err());
        let r = replies(&run(
            &mut v,
            &mut s,
            b"@mosaico photo-commit first.jpg\n",
            chunk,
        ));
        assert!(r[0].contains("not staged"));
    }
}

#[test]
fn failed_photo_crc_disconnect_and_album_clear_cannot_publish_partial_data() {
    let mut v = vfs();
    v.create_dir_all(PHOTO_ROOT).unwrap();
    v.write_file(&format!("{PHOTO_ROOT}/first.jpg"), b"old")
        .unwrap();
    let mut s = Session::new();
    let r = replies(&run(&mut v, &mut s, b"@mosaico photo-begin\n@mosaico file first.jpg 3 00000000\nnew@mosaico photo-commit first.jpg\n", 7));
    assert!(r.iter().any(|r| r.contains("crc")));
    assert!(r.last().unwrap().contains("transfer failed"));
    // Model a failed unlink leaving bytes behind after a CRC error. A
    // later commit must still require a successful transfer receipt.
    v.write_file(&format!("{PHOTO_STAGING}/photo/first.jpg"), b"bad")
        .unwrap();
    let r = replies(&run(
        &mut v,
        &mut s,
        b"@mosaico photo-commit first.jpg\n",
        64,
    ));
    assert!(r[0].contains("transfer failed"));
    assert_eq!(
        v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
        b"old"
    );
    run(
        &mut v,
        &mut s,
        b"@mosaico photo-begin\n@mosaico file first.jpg 20 00000000\npartial",
        7,
    );
    s.reset(&mut v);
    assert!(v.stat(PHOTO_STAGING).is_err());
    assert_eq!(
        v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
        b"old"
    );
    let mut input = b"@mosaico photo-begin\n".to_vec();
    input.extend(file_cmd("dated/new.JPEG", b"complete"));
    run(&mut v, &mut s, &input, 64);
    // N7 clear-data removes appdata, including the live transfer's staging.
    v.remove_tree("/data/appdata/photo").unwrap();
    let r = replies(&run(
        &mut v,
        &mut s,
        b"@mosaico photo-commit dated/new.JPEG\n",
        64,
    ));
    assert!(r[0].contains("transfer failed"));
    assert!(v.stat("/data/appdata/photo").is_err());
}

#[test]
fn photo_paths_kind_and_management_barrier_are_enforced() {
    let mut v = vfs();
    let mut s = Session::new();
    run(&mut v, &mut s, b"@mosaico photo-begin\n", 64);
    for path in ["../escape.jpg", "/data/other.jpg", "x.lua", "hidden/.x.jpg"] {
        let r = replies(&run(
            &mut v,
            &mut s,
            format!("@mosaico file {path} 1 00000000\n").as_bytes(),
            64,
        ));
        assert!(r[0].starts_with("@err"), "{r:?}");
    }
    let r = replies(&run(
        &mut v,
        &mut s,
        b"@mosaico commit photo\n@mosaico brain-commit\n",
        64,
    ));
    assert!(r.iter().all(|r| r.starts_with("@err")));
    s.set_management_busy(true);
    let r = replies(&run(
        &mut v,
        &mut s,
        b"@mosaico photo-begin\n@mosaico photo-commit x.jpg\n",
        64,
    ));
    assert_eq!(r, ["@err app management busy", "@err app management busy"]);
    s.set_management_busy(false);
    v.create_dir_all("/data/apps/.clear/photo").unwrap();
    let r = replies(&run(&mut v, &mut s, b"@mosaico photo-begin\n", 64));
    assert!(r[0].contains("pending recovery"));
}

#[test]
fn brain_transfer_is_scoped_and_commit_is_deferred_before_next_command() {
    use {crate::companion::brain::store as brain_store, crate::companion::brain::store::Action};
    for chunk in [1, 64, 512] {
        let mut v = vfs();
        brain_store::persist(&mut v, Some("old source")).unwrap();
        let mut session = Session::new();
        let mut stream = b"@mosaico brain-begin\n".to_vec();
        stream.extend(file_cmd("source.lua", MAIN));
        let replies = replies(&run(&mut v, &mut session, &stream, chunk));
        assert_eq!(
            replies,
            [
                "@ok brain-begin",
                "@ok ready source.lua",
                "@ok file source.lua"
            ]
        );
        let packet = b"@mosaico brain-commit\n@mosaico brain-status\n";
        let mut effects = Vec::new();
        let used = session.feed_until_deferred(&mut v, packet, &mut |e| effects.push(e));
        assert_eq!(&packet[used..], b"@mosaico brain-status\n");
        assert_eq!(
            effects,
            [Effect::Brain(Action::Install(
                String::from_utf8(MAIN.to_vec()).unwrap()
            ))]
        );
        assert_eq!(
            brain_store::selected(&mut v).unwrap().as_deref(),
            Some("old source"),
            "transport never activates a VM or overwrites active source"
        );
        assert!(v.stat(brain_store::STAGED_SOURCE).is_err());
        assert!(run(&mut v, &mut session, b"@mosaico brain-commit\n", chunk)
            .iter()
            .all(|e| matches!(e, Effect::Reply(s) if s.starts_with("@err"))));
    }
}

#[test]
fn bad_or_interrupted_brain_upload_cannot_touch_active_source() {
    use crate::companion::brain::store as brain_store;
    let mut v = vfs();
    brain_store::persist(&mut v, Some("old source")).unwrap();
    let mut session = Session::new();
    let effects = run(&mut v, &mut session, b"@mosaico brain-begin\n@mosaico file other.lua 1 0\n@mosaico file source.lua 0 0\n@mosaico file source.lua 65537 0\n@mosaico file source.lua 3 0\nabc@mosaico brain-commit\n", 64);
    let text = replies(&effects);
    assert!(text[1..4].iter().all(|e| e.starts_with("@err")));
    assert!(text[5].contains("crc"));
    assert!(text[6].starts_with("@err"));
    run(
        &mut v,
        &mut session,
        b"@mosaico file source.lua 30 0\npartial",
        64,
    );
    assert_eq!(v.open_files(), 1);
    session.reset(&mut v);
    assert_eq!(v.open_files(), 0);
    assert_eq!(
        brain_store::selected(&mut v).unwrap().as_deref(),
        Some("old source")
    );
    assert!(v.stat(brain_store::STAGING).is_err());
    // A later app upload has its own staging root and cannot be committed
    // through the brain command, even when its app id is "resident".
    run(&mut v, &mut session, b"@mosaico begin resident\n", 64);
    assert!(
        replies(&run(&mut v, &mut session, b"@mosaico brain-commit\n", 64))[0].starts_with("@err")
    );
}

#[test]
fn push_commits_app_at_any_chunking() {
    for chunk in [1, 7, 64, 512, 1 << 20] {
        let mut v = vfs();
        let mut sess = Session::new();
        let effects = run(&mut v, &mut sess, &push_stream(true), chunk);
        assert_eq!(
            replies(&effects),
            [
                "@ok hello mosaico-push 1",
                "@ok begin hello",
                "@ok ready launcher.json",
                "@ok file launcher.json",
                "@ok ready scripts/main.lua",
                "@ok file scripts/main.lua",
                "@ok ready empty.txt",
                "@ok file empty.txt",
                "@ok commit hello"
            ],
            "chunk {chunk}"
        );
        assert!(effects.contains(&Effect::Reload {
            run: Some("hello".into())
        }));
        assert_eq!(
            v.read_file("/data/apps/hello/scripts/main.lua").unwrap(),
            MAIN
        );
        assert_eq!(v.read_file("/data/apps/hello/empty.txt").unwrap(), b"");
        assert!(v.stat(STAGING).is_err(), "staging is cleaned up");
        let reg = Registry::scan(&mut v, &mut IconCache::new(), &mut |_| {});
        let i = reg.find("hello").expect("listed");
        assert!(reg.apps[i].runnable());
        assert_eq!(reg.apps[i].title, "Hi");
    }
}

#[test]
fn commit_replaces_old_files_and_rm_removes() {
    let mut v = vfs();
    v.create_dir_all("/data/apps/hello/old").unwrap();
    v.write_file("/data/apps/hello/old/stale.lua", b"x")
        .unwrap();
    let mut sess = Session::new();
    run(&mut v, &mut sess, &push_stream(false), 512);
    assert!(v.stat("/data/apps/hello/old/stale.lua").is_err());
    let e = run(&mut v, &mut sess, b"@mosaico rm hello\n", 512);
    assert!(
        replies(&e).is_empty(),
        "queueing must never claim completion"
    );
    assert!(
        v.stat("/data/apps/hello").is_ok(),
        "feed cannot delete a running app"
    );
    let Effect::Manage(op) = e[0] else {
        panic!("expected deferred operation")
    };
    crate::apps::lifecycle::uninstall::execute(&mut v, op).unwrap();
    assert!(v.stat("/data/apps/hello").is_err());
    let r = replies(&run(
        &mut v,
        &mut sess,
        b"@mosaico ls\n@mosaico rm hello\n",
        512,
    ));
    assert_eq!(r[0], "@ok ls");
    assert!(r[1].starts_with("@err app hello not found"));
}

#[test]
fn partial_upload_ui_removal_cannot_resurrect_app_or_commit() {
    let mut v = vfs();
    let mut session = Session::new();
    run(&mut v, &mut session, &push_stream(false), 512);
    run(
        &mut v,
        &mut session,
        b"@mosaico begin hello\n@mosaico file main.lua 6 0\nabc",
        512,
    );
    assert_eq!(v.open_files(), 1);
    let identity = crate::apps::lifecycle::uninstall::resolve(&mut v, "hello").unwrap();
    crate::apps::lifecycle::uninstall::execute(
        &mut v,
        crate::apps::lifecycle::uninstall::Operation::Remove(identity),
    )
    .unwrap();
    let e = run(
        &mut v,
        &mut session,
        b"def@mosaico commit hello\n@mosaico file resurrect.lua 0 0\n",
        512,
    );
    assert!(
        replies(&e).iter().all(|r| r.starts_with("@err")),
        "{:?}",
        replies(&e)
    );
    assert_eq!(v.open_files(), 0);
    assert!(v.stat("/data/apps/hello").is_err());
    assert!(v.stat("/data/apps/.incoming/hello").is_err());
}
#[test]
fn deferred_boundary_preserves_single_packet_tail_and_payload_batching() {
    for chunk in [64, 512] {
        let mut v = vfs();
        let mut session = Session::new();
        run(&mut v, &mut session, &push_stream(false), 512);
        run(&mut v, &mut session, b"@mosaico begin hello\n", 512);
        let packet = b"@mosaico rm hello\n@mosaico commit hello run\n";
        let mut effects = Vec::new();
        let consumed = session.feed_until_deferred(&mut v, packet, &mut |e| effects.push(e));
        assert_eq!(&packet[consumed..], b"@mosaico commit hello run\n");
        let Effect::Manage(op) = effects[0] else {
            panic!("expected deferred delete")
        };
        crate::apps::lifecycle::uninstall::execute(&mut v, op).unwrap();
        effects.clear();
        session.feed_until_deferred(&mut v, &packet[consumed..], &mut |e| effects.push(e));
        assert!(replies(&effects)[0].starts_with("@err"));
        assert!(v.stat("/data/apps/hello").is_err());
        run(&mut v, &mut session, b"@mosaico begin payload\n", 512);
        let payload = vec![42u8; 4096];
        let header = format!("@mosaico file big.bin 4096 {:08x}\n", crc32(&payload));
        run(&mut v, &mut session, header.as_bytes(), 512);
        session.payload_writes.clear();
        run(&mut v, &mut session, &payload, chunk);
        assert_eq!(
            session.payload_writes,
            vec![chunk; 4096 / chunk],
            "one write per USB block"
        );
    }
}

#[test]
fn management_barrier_rejects_navigation_and_mutations_explicitly() {
    let mut v = vfs();
    let mut session = Session::new();
    session.set_management_busy(true);
    let e = run(&mut v, &mut session, b"@mosaico run snake\n@mosaico begin hello\n@mosaico rmdata snake\n@mosaico commit hello run\n", 512);
    assert_eq!(replies(&e), ["@err app management busy"; 4]);
    assert!(e.iter().all(|e| matches!(e, Effect::Reply(_))));
}

#[test]
fn bad_crc_and_paths_are_rejected() {
    let mut v = vfs();
    let mut sess = Session::new();
    let mut s = b"@mosaico file a.lua 1 0\n@mosaico begin hello\n".to_vec();
    s.extend_from_slice(
        format!("@mosaico file scripts/main.lua {} 12345678\n", MAIN.len()).as_bytes(),
    );
    s.extend_from_slice(MAIN);
    for bad in [
        "../x", ".hidden", "/abs", "a//b", "a/../b", "sp ace", "a\\b",
    ] {
        s.extend_from_slice(format!("@mosaico file {bad} 0 0\n").as_bytes());
    }
    s.extend_from_slice(format!("@mosaico file big.bin {} 0\n", FILE_MAX + 1).as_bytes());
    s.extend_from_slice(b"@mosaico commit other\n@mosaico commit hello\n@mosaico frobnicate\n");
    let r = replies(&run(&mut v, &mut sess, &s, 5));
    assert!(r[0].starts_with("@err file before begin"), "{r:?}");
    assert_eq!(r[1], "@ok begin hello");
    assert_eq!(r[2], "@ok ready scripts/main.lua");
    let r = &r[1..];
    assert!(r[2].contains("crc"), "{r:?}");
    assert!(v
        .stat("/data/apps/.incoming/hello/scripts/main.lua")
        .is_err());
    // "sp ace" splits into four words and is a bad command.
    assert!(r[3..10].iter().all(|t| t.starts_with("@err")), "{r:?}");
    assert!(r[10].contains("larger"), "{r:?}");
    assert!(r[11].contains("not staged"));
    assert!(r[12].contains("launcher.json"), "no manifest: {r:?}");
    assert!(r[13].starts_with("@err bad command"));
    assert_eq!(r.len(), 14);
    assert!(v.stat("/data/apps/hello").is_err());
}

#[test]
fn long_lines_reset_and_controls() {
    let mut v = vfs();
    let mut sess = Session::new();
    let mut s = vec![b'a'; 1000];
    s.extend_from_slice(
        b"\n@mosaico run snake\n@mosaico stop\n@mosaico reload\n@mosaico begin x\n",
    );
    let e = run(&mut v, &mut sess, &s, 100);
    assert_eq!(replies(&e), ["@err line too long; reconnect required"]);
    assert_eq!(e.len(), 2);
    assert!(e.contains(&Effect::OtaCancel));
    assert!(v.stat("/data/apps/.incoming/x").is_err());
    assert!(run(
        &mut v,
        &mut sess,
        b"@mosaico run snake\n@mosaico stop\n",
        100
    )
    .is_empty());
    sess.reset(&mut v);
    let e = run(
        &mut v,
        &mut sess,
        b"@mosaico run snake\n@mosaico stop\n@mosaico reload\n@mosaico begin x\n",
        100,
    );
    let r = replies(&e);
    assert_eq!(
        r,
        ["@ok run snake", "@ok stop", "@ok reload", "@ok begin x"]
    );
    assert!(e.contains(&Effect::Run("snake".into())) && e.contains(&Effect::Stop));
    assert!(v.stat("/data/apps/.incoming/x").is_ok());
    // A half-sent file, then the host disconnects.
    run(&mut v, &mut sess, b"@mosaico file a.lua 10 0\nabc", 100);
    sess.reset(&mut v);
    assert!(v.stat(STAGING).is_err());
    let r = replies(&run(&mut v, &mut sess, b"@mosaico hello\n", 100));
    assert_eq!(r, ["@ok hello mosaico-push 1"]);
}

#[test]
fn status_and_reboot() {
    let mut v = vfs();
    let mut sess = Session::new();
    let e = run(&mut v, &mut sess, b"@mosaico status\n@mosaico reboot\n", 64);
    let r = replies(&e);
    assert!(
        r[0].starts_with("@ok status ") && r[0].contains(" data:62/64x4096"),
        "{r:?}"
    );
    assert!(r[0].contains(" system:"), "{r:?}");
    assert_eq!(r[1], "@ok reboot");
    assert!(e.contains(&Effect::Reboot));
    let mut empty = Vfs::new();
    let r = replies(&run(&mut empty, &mut sess, b"@mosaico status\n", 64));
    assert_eq!(r, ["@ok status no-mounts"]);
}

#[test]
fn production_protocol_rejects_removed_debug_commands() {
    for command in [
        "cpu-status",
        "wifi-probe example.com 80",
        "camera-probe",
        "audio-tone 1000 100",
        "audio-loopback /data/test.wav",
        "nand-read 0",
        "nand-test write",
        "nand-bench write",
        "imu-irq int1",
        "imu-irq-status",
        "usb-diag-reset-arm",
        "usb-diag-reset-cancel",
    ] {
        for chunk in [1, 64, 512] {
            let effects = run(
                &mut Vfs::new(),
                &mut Session::new(),
                format!("@mosaico {command}\n").as_bytes(),
                chunk,
            );
            assert_eq!(effects.len(), 1, "{command}");
            assert!(
                matches!(&effects[0], Effect::Reply(text) if text.starts_with("@err")),
                "{command}: {effects:?}"
            );
        }
    }
}
