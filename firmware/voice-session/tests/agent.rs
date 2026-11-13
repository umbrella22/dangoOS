use fs::{Fs, Geometry, RamDevice, Vfs};
use serde_json::{json, Value};
use voice_session::{
    agent::Action,
    storage::{Session, PATH},
};
fn vfs() -> Vfs {
    let mut v = Vfs::new();
    v.mount(
        "data",
        Fs::format(Box::new(RamDevice::new(4096, 1024)), Geometry::flash(1024)).unwrap(),
    );
    v
}
fn event(s: &mut Session, v: &mut Vfs, index: u64, mut e: Value) {
    e["sequence_number"] = index.into();
    let wire = format!("data: {e}\r\n\r\n");
    for chunk in wire.as_bytes().chunks(43) {
        s.feed(v, chunk).unwrap();
    }
}
fn start(s: &mut Session, v: &mut Vfs) {
    event(
        s,
        v,
        0,
        json!({"type":"response.created","response":{"id":"r","status":"in_progress","output":[]}}),
    );
}
fn completed(s: &mut Session, v: &mut Vfs, index: u64, output: Value) {
    event(
        s,
        v,
        index,
        json!({"type":"response.completed","response":{"id":"r","status":"completed","error":null,"incomplete_details":null,"output":output}}),
    );
}
fn call(id: &str, name: &str, args: &str) -> Value {
    json!({"type":"function_call","id":format!("item_{id}"),"status":"completed","call_id":id,"name":name,"arguments":args})
}
fn context(s: &mut Session, v: &mut Vfs) -> Value {
    let agent = s.agent().unwrap();
    let size = agent.context_bytes(v).unwrap();
    let mut raw = vec![0; size as usize];
    let mut at = 0;
    while at < raw.len() {
        at += agent.read_context(v, at as u32, &mut raw[at..]).unwrap();
    }
    serde_json::from_slice(format!("[{}]", String::from_utf8(raw).unwrap()).as_bytes()).unwrap()
}
#[test]
fn tool_round_replays_large_reasoning_and_identifiers_from_files_and_retains_one_visible_reply() {
    let mut v = vfs();
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    s.enable_agent(&mut v, br#"{"role":"user","content":"quiet"}"#)
        .unwrap();
    start(&mut s, &mut v);
    assert!(s.agent().unwrap().next_call(&mut v).is_err());
    let id = "调用🦊\"\\".repeat(400);
    let thought = "thinking 你好\n".repeat(20000);
    let args = format!(
        "{{{}\"percent\"{}:{}50{}}}",
        " ".repeat(20000),
        " ".repeat(3000),
        " ".repeat(2000),
        " ".repeat(7000)
    );
    let output = json!([{"type":"reasoning","content":[{"type":"reasoning_text","text":thought}]},call(&id,"set_volume",&args),call("read","device_status","{}")]);
    completed(&mut s, &mut v, 1, output.clone());
    assert_eq!(s.reply.function_calls, 2);
    assert!(s.buffered_bytes() < 20 * 1024);
    assert_eq!(context(&mut s, &mut v)[1], output[0]);
    let first = s.agent().unwrap().next_call(&mut v).unwrap().unwrap();
    assert_eq!(first.action, Ok(Action::Volume(50)));
    assert!(s.agent().unwrap().next_call(&mut v).is_err());
    s.agent()
        .unwrap()
        .result(&mut v, first, br#""volume configured""#)
        .unwrap();
    let second = s.agent().unwrap().next_call(&mut v).unwrap().unwrap();
    assert_eq!(second.action, Ok(Action::Status));
    s.agent()
        .unwrap()
        .result(&mut v, second, br#""status""#)
        .unwrap();
    assert!(s.agent().unwrap().next_call(&mut v).unwrap().is_none());
    let body = context(&mut s, &mut v);
    assert_eq!(body[4]["call_id"], id);
    assert_eq!(body[5]["call_id"], "read");
    s.next_round().unwrap();
    start(&mut s, &mut v);
    event(
        &mut s,
        &mut v,
        1,
        json!({"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":"已调低。"}),
    );
    event(
        &mut s,
        &mut v,
        2,
        json!({"type":"response.output_text.done","item_id":"m","output_index":0,"content_index":0,"text":"已调低。"}),
    );
    completed(
        &mut s,
        &mut v,
        3,
        json!([{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"已调低。"}]}]),
    );
    assert_eq!(s.window().unwrap().text, "已调低。");
    s.close(&mut v).unwrap();
    assert_eq!(v.open_files(), 0);
    assert_eq!(v.read_file(PATH).unwrap(), "已调低。".as_bytes());
    assert!(v.stat("/data/voice/context.json").is_err());
}
#[test]
fn tool_arguments_follow_the_device_schema_and_unknown_calls_receive_errors() {
    let mut v = vfs();
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    s.enable_agent(&mut v, b"{}").unwrap();
    start(&mut s, &mut v);
    let cases = [
        ("set_volume", r#"{"percent":0}"#, Ok(Action::Volume(0))),
        (
            "set_volume",
            r#"{"\u0070ercent":100}"#,
            Ok(Action::Volume(100)),
        ),
        (
            "set_volume",
            r#"{"percent":1.0e2}"#,
            Ok(Action::Volume(100)),
        ),
        (
            "set_volume",
            r#"{"percent":10.000}"#,
            Ok(Action::Volume(10)),
        ),
        ("set_volume", r#"{"percent":-0.0}"#, Ok(Action::Volume(0))),
        (
            "set_volume",
            r#"{"percent":0.001e3}"#,
            Ok(Action::Volume(1)),
        ),
        (
            "set_volume",
            r#"{"percent":10.5}"#,
            Err("invalid_arguments"),
        ),
        ("set_volume", r#"{"percent":101}"#, Err("invalid_arguments")),
        (
            "set_volume",
            r#"{"percent":50,"percent":60}"#,
            Err("invalid_arguments"),
        ),
        (
            "set_volume",
            r#"{"percent":"50"}"#,
            Err("invalid_arguments"),
        ),
        ("device_status", "[]", Err("invalid_arguments")),
        (
            "device_status",
            r#"{"percent":50}"#,
            Err("invalid_arguments"),
        ),
        (
            "set_brightness",
            r#"{"percent":9}"#,
            Err("brightness_requires_10_to_100"),
        ),
        ("shell", "{}", Err("unknown_tool")),
    ];
    let output = cases
        .iter()
        .enumerate()
        .map(|(i, (name, args, _))| call(&i.to_string(), name, args))
        .collect::<Vec<_>>();
    completed(&mut s, &mut v, 1, json!(output));
    for (_, _, expected) in cases {
        let c = s.agent().unwrap().next_call(&mut v).unwrap().unwrap();
        assert_eq!(c.action, expected);
        s.agent()
            .unwrap()
            .result(&mut v, c, br#""result""#)
            .unwrap();
    }
    s.close(&mut v).unwrap();
    assert_eq!(v.open_files(), 0);
}
#[test]
fn cancellation_removes_working_context_and_new_tasks_replace_the_previous_reply() {
    let mut v = vfs();
    let mut s = Session::new(&mut v, |_| {}).unwrap();
    s.enable_agent(&mut v, b"{}").unwrap();
    start(&mut s, &mut v);
    event(
        &mut s,
        &mut v,
        1,
        json!({"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":"partial"}),
    );
    s.close(&mut v).unwrap();
    assert_eq!(v.open_files(), 0);
    assert_eq!(v.read_file(PATH).unwrap(), b"partial");
    assert!(v.stat("/data/voice/output.json").is_err());
    let s = Session::new(&mut v, |_| {}).unwrap();
    s.close(&mut v).unwrap();
    assert!(v.read_file(PATH).unwrap().is_empty());
}
