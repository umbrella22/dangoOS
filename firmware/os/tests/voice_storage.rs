//! NAND-format file lifecycle and the production incremental receive path.
use os::services::voice::{
    storage::{self, Session},
    WINDOW_BYTES,
};
use serde_json::{json, Value};

fn vfs(blocks: u32) -> fs::Vfs {
    let fs = fs::Fs::format(
        Box::new(fs::RamDevice::new(4096, blocks)),
        fs::Geometry::flash(blocks),
    )
    .unwrap();
    let mut v = fs::Vfs::new();
    v.mount("data", fs);
    v
}
fn event(index: u64, mut value: Value) -> Vec<u8> {
    value["sequence_number"] = index.into();
    format!("data: {value}\r\n\r\n").into_bytes()
}
fn start(s: &mut Session, v: &mut fs::Vfs) {
    s.feed(
        v,
        &event(
            0,
            json!({"type":"response.created","response":{"id":"r","status":"in_progress"}}),
        ),
    )
    .unwrap();
}
fn delta(index: u64, text: &str) -> Vec<u8> {
    event(
        index,
        json!({"type":"response.output_text.delta","item_id":"m","output_index":1,"content_index":0,"delta":text}),
    )
}
fn done(index: u64, text: &str, thought: &str) -> Vec<u8> {
    event(
        index,
        json!({"type":"response.completed","response":{"id":"r","status":"completed","error":null,"incomplete_details":null,"output":[{"type":"reasoning","content":[{"type":"reasoning_text","text":thought}]},{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]}]}}),
    )
}
fn text_done(index: u64, text: &str) -> Vec<u8> {
    event(
        index,
        json!({"type":"response.output_text.done","item_id":"m","output_index":1,"content_index":0,"text":text}),
    )
}

#[test]
fn replies_and_snapshots_exceed_old_limits_while_ram_and_pages_stay_small() {
    let mut v = vfs(1024);
    let path = storage::PATH;
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    start(&mut s, &mut v);
    let thought = "reasoning only ".repeat(24000);
    let reasoning = event(
        1,
        json!({"type":"response.reasoning_text.delta","delta":thought}),
    );
    let mut peak = 0;
    for chunk in reasoning.chunks(509) {
        s.feed(&mut v, chunk).unwrap();
        peak = peak.max(s.buffered_bytes());
    }
    assert_eq!(s.total(), 0);
    let text = "团子🦊 says \"hello\" \\\n".repeat(5000);
    let mut index = 2;
    let mut at = 0;
    while at < text.len() {
        let mut end = (at + 1024).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let wire = delta(index, &text[at..end]);
        index += 1;
        for chunk in wire.chunks(251) {
            s.feed(&mut v, chunk).unwrap();
            peak = peak.max(s.buffered_bytes());
        }
        at = end;
    }
    for wire in [text_done(index, &text), done(index + 1, &text, &thought)] {
        assert!(wire.len() > 16 * 1024);
        for chunk in wire.chunks(4019) {
            s.feed(&mut v, chunk).unwrap();
            peak = peak.max(s.buffered_bytes());
        }
    }
    assert!(s.reply.done);
    assert_eq!(s.total(), text.len());
    let tail = s.window().unwrap();
    assert!(tail.text.len() <= WINDOW_BYTES);
    assert_eq!(&text[tail.start..], tail.text);
    assert!(peak < 16 * 1024, "dynamic parser buffers grew to {peak}");
    s.close(&mut v).unwrap();
    storage::finish(&mut v, path, text.len(), "done").unwrap();
    assert_eq!(v.read_file(path).unwrap(), text.as_bytes());
    assert_eq!(v.read_file(&format!("{path}.state")).unwrap(), b"done");
    let mut pages = String::new();
    while pages.len() < text.len() {
        let page = storage::window(&mut v, path, pages.len(), text.len()).unwrap();
        assert_eq!(page.start, pages.len());
        assert!(page.text.len() <= WINDOW_BYTES);
        assert!(!page.text.is_empty());
        pages.push_str(&page.text);
    }
    assert_eq!(pages, text);
    let mut raw = Vec::new();
    while raw.len() < text.len() {
        raw.extend(storage::chunk(&mut v, path, raw.len(), text.len()).unwrap());
    }
    assert_eq!(raw, text.as_bytes());
    assert_eq!(v.open_files(), 0);
}
#[test]
fn cancellation_and_bad_terminal_keep_the_current_prefix_and_replace_previous_turns() {
    let mut v = vfs(128);
    let path = storage::PATH;
    v.create_dir_all("/data/voice").unwrap();
    v.write_file("/data/voice/old-session.txt", b"old reply")
        .unwrap();
    v.write_file("/data/voice/old-session.txt.state", b"done")
        .unwrap();
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    assert!(v.stat("/data/voice/old-session.txt").is_err());
    start(&mut s, &mut v);
    s.feed(&mut v, &delta(1, "已有正文🦊")).unwrap();
    let partial = delta(2, &"uncommitted ".repeat(500));
    s.feed(&mut v, &partial[..partial.len() / 2]).unwrap();
    assert_eq!(s.window().unwrap().text, "已有正文🦊");
    let accepted = s.total();
    s.close(&mut v).unwrap();
    storage::finish(&mut v, path, accepted, "cancelled").unwrap();
    assert_eq!(v.read_file(path).unwrap(), "已有正文🦊".as_bytes());
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    assert!(v.read_file(path).unwrap().is_empty());
    assert_eq!(
        v.read_file(&format!("{path}.state")).unwrap(),
        b"replying\n"
    );
    assert_eq!(v.read_dir("/data/voice").unwrap().len(), 2);
    start(&mut s, &mut v);
    s.feed(&mut v, &delta(1, "恢复正常")).unwrap();
    s.feed(&mut v, &text_done(2, "恢复正常")).unwrap();
    assert!(s.feed(&mut v, &done(3, "错误快照", "")).is_err());
    assert!(!s.reply.done);
    s.close(&mut v).unwrap();
    assert_eq!(v.read_file(path).unwrap(), "恢复正常".as_bytes());
    assert_eq!(v.open_files(), 0);
}
#[test]
fn escaped_unicode_and_malformed_json_are_independent_of_network_boundaries() {
    let mut v = vfs(64);
    let path = storage::PATH;
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    start(&mut s, &mut v);
    let wire=br#"data: {"type":"response.output_text.delta","delta":"\u56e2\u5b50\ud83e\udd8a\n\"\\","item_id":"m","output_index":1,"content_index":0,"sequence_number":1}

"#;
    for b in wire {
        s.feed(&mut v, &[*b]).unwrap();
    }
    let text = "团子🦊\n\"\\";
    assert_eq!(s.total(), text.len());
    s.feed(&mut v, &text_done(2, text)).unwrap();
    s.feed(&mut v, &done(3, text, "")).unwrap();
    s.close(&mut v).unwrap();
    assert_eq!(v.read_file(path).unwrap(), text.as_bytes());
    for bad in [
        "{\"type\":\"response.created\",}",
        "{]",
        "{\"type\":\"\\ud800\"}",
        "{\"type\":\"a\" \"sequence_number\":0}",
        "{\"sequence_number\":01}",
    ] {
        let mut reply = os::services::voice::protocol::Reply::default();
        assert!(reply.event(bad.as_bytes()).is_err(), "accepted {bad}");
    }
}
#[test]
fn storage_exhaustion_returns_an_error_and_retains_a_committed_prefix() {
    let mut v = vfs(32);
    let path = storage::PATH;
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    start(&mut s, &mut v);
    let block = "x".repeat(4096);
    let mut failed = false;
    for index in 1..100 {
        if s.feed(&mut v, &delta(index, &block)).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
    assert!(!s.reply.done);
    let total = s.total();
    assert!(total > 0);
    let _ = s.close(&mut v);
    let file = v.read_file(path).unwrap();
    assert!(file.len() >= total);
    assert!(file[..total].iter().all(|b| *b == b'x'));
    assert_eq!(v.open_files(), 0);
}

#[test]
fn tiny_deltas_and_live_windows_do_not_program_flash_for_every_character() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Counted {
        inner: fs::RamDevice,
        programs: Arc<AtomicUsize>,
    }
    impl fs::BlockDevice for Counted {
        fn read(&mut self, block: u32, off: u32, out: &mut [u8]) -> Result<(), fs::Errno> {
            self.inner.read(block, off, out)
        }
        fn prog(&mut self, block: u32, off: u32, bytes: &[u8]) -> Result<(), fs::Errno> {
            self.programs.fetch_add(1, Ordering::Relaxed);
            self.inner.prog(block, off, bytes)
        }
        fn erase(&mut self, block: u32) -> Result<(), fs::Errno> {
            self.inner.erase(block)
        }
        fn sync(&mut self) -> Result<(), fs::Errno> {
            self.inner.sync()
        }
    }
    let programs = Arc::new(AtomicUsize::new(0));
    let fs = fs::Fs::format(
        Box::new(Counted {
            inner: fs::RamDevice::new(4096, 128),
            programs: Arc::clone(&programs),
        }),
        fs::Geometry::flash(128),
    )
    .unwrap();
    let mut v = fs::Vfs::new();
    v.mount("data", fs);
    let path = storage::PATH;
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    start(&mut s, &mut v);
    programs.store(0, Ordering::Relaxed);
    let text = "团".repeat(1500);
    for index in 1..=1500 {
        s.feed(&mut v, &delta(index, "团")).unwrap();
        let window = s.window().unwrap();
        assert!(window.text.len() <= WINDOW_BYTES);
        assert_eq!(window.text, text[window.start..index as usize * 3]);
    }
    s.feed(&mut v, &text_done(1501, &text)).unwrap();
    s.feed(&mut v, &done(1502, &text, "")).unwrap();
    s.close(&mut v).unwrap();
    assert_eq!(v.read_file(path).unwrap(), text.as_bytes());
    let writes = programs.load(Ordering::Relaxed);
    assert!(
        writes < 128,
        "1500 small deltas caused {writes} flash programs"
    );
    assert_eq!(v.open_files(), 0);
}

#[test]
fn desktop_visible_stream_is_complete_on_disk_and_invalid_chunks_do_not_advance() {
    let mut v = vfs(1024);
    let mut session = Session::new(&mut v, |_| {}).unwrap();
    let text = "团子🦊桌面回复\n".repeat(20000);
    let baseline = session.buffered_bytes();
    let phrase = "团子🦊桌面回复\n".as_bytes();
    for _ in 0..20000 {
        let offset = session.total();
        session.append_plain(&mut v, offset, phrase).unwrap();
        assert!(session.buffered_bytes() <= baseline + 2 * WINDOW_BYTES);
    }
    let offset = session.total();
    assert!(session.append_plain(&mut v, offset + 1, b"x").is_err());
    assert!(session.append_plain(&mut v, offset, b"\xff").is_err());
    assert!(session.append_plain(&mut v, offset, b"x\0y").is_err());
    assert_eq!(session.total(), text.len());
    let window = session.window().unwrap();
    assert_eq!(window.text, &text[window.start..]);
    session.close(&mut v).unwrap();
    assert_eq!(v.open_files(), 0);
    assert_eq!(v.read_file(storage::PATH).unwrap(), text.as_bytes());
}
