//! Offline wire integration: real capture FIFO, WS/ASR contracts, HTTP chunk
//! framing and SSE parsing. Physical I2S and provider acceptance are separate.
use base64::{engine::general_purpose::STANDARD, Engine};
use embedded_io_async::Read;
use os::{
    formats::http::Response,
    services::voice::{
        capture::{Buffer, Downsample, PACKET_BYTES},
        config::Config,
        protocol::{self, Reply, Transcript},
        sse, websocket,
    },
};
use serde_json::{json, Value};

struct Fragments<'a>(&'a [u8], usize);
impl embedded_io::ErrorType for Fragments<'_> {
    type Error = core::convert::Infallible;
}
impl Read for Fragments<'_> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        let n = self.0.len().min(out.len()).min(self.1);
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}
fn run<T>(f: impl core::future::Future<Output = T>) -> T {
    let mut f = core::pin::pin!(f);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    match f.as_mut().poll(&mut cx) {
        core::task::Poll::Ready(v) => v,
        _ => panic!("memory IO unexpectedly suspended"),
    }
}
fn config() -> Config {
    Config::parse(&serde_json::to_vec(&json!({"schema":1,"asr":{"url":"wss://asr.example/v1/realtime?intent=transcription","api_key":"asr-key","model":"transcribe"},
        "chat":{"url":"https://chat.example/v1/responses","api_key":"chat-key","model":"chat"}})).unwrap()).unwrap()
}
fn server_frame(text: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x81];
    if text.len() < 126 {
        bytes.push(text.len() as u8);
    } else {
        bytes.push(126);
        bytes.extend_from_slice(&(text.len() as u16).to_be_bytes());
    }
    bytes.extend_from_slice(text);
    bytes
}
#[test]
fn microphone_tail_is_uploaded_before_commit_and_committed_transcript_becomes_responses_input() {
    let mut input: Vec<u8> = (0..48_000)
        .flat_map(|n| [(n % 321) as u8, 0, 0, 0])
        .collect();
    let n = Downsample::default().convert_in_place(&mut input);
    let pcm = &input[..n];
    let mut q = Buffer::new().unwrap();
    q.start(41).unwrap();
    q.push(41, &pcm[..pcm.len() - 2]).unwrap();
    q.request_finish(41);
    q.push(41, &pcm[pcm.len() - 2..]).unwrap();
    assert!(!q.drained(41));
    q.close(41, None);
    let mut packet = vec![0; PACKET_BYTES];
    let mut text = String::new();
    let mut wire = Vec::new();
    let mut server_pcm = Vec::new();
    while !q.drained(41) {
        let n = q.pop(41, &mut packet).unwrap();
        protocol::append_audio(&packet[..n], &mut text);
        websocket::encode(1, text.as_bytes(), [19, 27, 31, 37], &mut wire).unwrap();
        assert_eq!(wire[0], 0x81);
        assert_ne!(wire[1] & 0x80, 0);
        let at = if wire[1] & 127 == 126 { 4 } else { 2 };
        let mask = &wire[at..at + 4];
        let raw: Vec<u8> = wire[at + 4..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(value["type"], "input_audio_buffer.append");
        server_pcm.extend(STANDARD.decode(value["audio"].as_str().unwrap()).unwrap());
    }
    assert_eq!(server_pcm, pcm);
    assert_eq!(q.milliseconds(), 1000);
    let mut t = Transcript::default();
    t.sent_commit = true;
    let events = [
        json!({"type":"session.updated"}),
        json!({"type":"input_audio_buffer.committed","item_id":"turn-41"}),
        json!({"type":"conversation.item.input_audio_transcription.delta","item_id":"turn-41","delta":"你好"}),
        json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"turn-41","transcript":"你好，团子"}),
    ];
    let mut decoder = websocket::Decoder::default();
    for event in events {
        for byte in server_frame(event.to_string().as_bytes()) {
            if let Some(websocket::Event::Text(text)) = decoder.push(byte).unwrap() {
                t.event(text.as_bytes()).unwrap();
            }
        }
    }
    assert!(t.completed && t.updated);
    let c = config();
    let context = serde_json::json!({"type":"message","role":"user","content":t.text}).to_string();
    let body = format!(
        "{}{}{}",
        protocol::responses::prefix(&c),
        context,
        protocol::responses::suffix()
    );
    let request = format!("{}{}", protocol::responses::head(&c, body.len()), body);
    let (headers, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(headers.contains("Authorization: Bearer chat-key\r\n"));
    assert!(!headers.contains("asr-key"));
    assert!(headers.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    assert!(headers.contains(&format!("Content-Length: {}\r\n", body.len())));
    let v: Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["input"][0]["content"], "你好，团子");
    assert_eq!(v["stream"], true);
    assert_eq!(v["store"], false);
}

fn responses_events() -> Vec<Value> {
    let reasoning = json!({"id":"rs","type":"reasoning","summary":[]});
    let parts = json!([{"type":"output_text","text":"你好，","annotations":[],"logprobs":[]},
        {"type":"output_text","text":"团子","annotations":[],"logprobs":[]}]);
    let message =
        json!({"id":"m","type":"message","role":"assistant","status":"completed","content":parts});
    vec![
        json!({"type":"response.created","response":{"id":"r","status":"in_progress","output":[]}}),
        json!({"type":"response.in_progress","response":{"id":"r","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":reasoning}),
        json!({"type":"response.reasoning_summary_text.delta","item_id":"rs","output_index":0,"summary_index":0,"delta":"not-visible"}),
        json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"id":"m","type":"message","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.content_part.added","item_id":"m","output_index":1,"content_index":0,"part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}}),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":1,"content_index":0,"delta":"你好","logprobs":[]}),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":1,"content_index":0,"delta":"，","logprobs":[]}),
        json!({"type":"response.output_text.done","item_id":"m","output_index":1,"content_index":0,"text":"你好，","logprobs":[]}),
        json!({"type":"response.content_part.done","item_id":"m","output_index":1,"content_index":0,"part":parts[0]}),
        json!({"type":"response.content_part.added","item_id":"m","output_index":1,"content_index":1,"part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}}),
        json!({"type":"response.output_text.delta","item_id":"m","output_index":1,"content_index":1,"delta":"团子","logprobs":[]}),
        json!({"type":"response.output_text.done","item_id":"m","output_index":1,"content_index":1,"text":"团子","logprobs":[]}),
        json!({"type":"response.content_part.done","item_id":"m","output_index":1,"content_index":1,"part":parts[1]}),
        json!({"type":"response.output_item.done","output_index":1,"item":message}),
        json!({"type":"response.completed","response":{"id":"r","status":"completed","error":null,"incomplete_details":null,"output":[reasoning,message],"usage":{"input_tokens":3,"output_tokens":5}}}),
    ]
}
fn sse_payload(events: &[Value]) -> Vec<u8> {
    let mut payload = b":keepalive\r\n\r\n".to_vec();
    for (index, event) in events.iter().enumerate() {
        let mut event = event.clone();
        event["sequence_number"] = index.into();
        payload.extend_from_slice(
            format!(
                "event: {}\r\ndata: {}\r\n\r\n",
                event["type"].as_str().unwrap(),
                event
            )
            .as_bytes(),
        );
    }
    payload
}

#[test]
fn reasoning_larger_than_the_visible_reply_budget_is_not_accumulated() {
    let reasoning = "private thought ".repeat(512);
    assert!(reasoning.len() > 4096);
    let mut events = responses_events();
    let item = json!({"id":"rs","type":"reasoning","summary":[],
        "content":[{"type":"reasoning_text","text":reasoning}]});
    events[2]["item"] = item.clone();
    events[4]["item"] = item.clone();
    events.last_mut().unwrap()["response"]["output"][0] = item;
    events.splice(
        3..4,
        reasoning.as_bytes().chunks(128).map(|chunk| {
            json!({"type":"response.reasoning_text.delta","item_id":"rs","output_index":0,
            "content_index":0,"delta":core::str::from_utf8(chunk).unwrap()})
        }),
    );
    let mut decoder = sse::Decoder::default();
    let mut reply = Reply::default();
    let mut visible = Vec::new();
    for byte in sse_payload(&events) {
        if let Some(data) = decoder.push(byte).unwrap() {
            if reply.event(data).unwrap() {
                visible.push(reply.text.clone());
            } else if visible.is_empty() {
                assert_eq!(reply.text.capacity(), 0);
            }
        }
    }
    assert!(reply.done);
    assert_eq!(reply.text, "你好，团子");
    assert_eq!(visible, ["你好", "你好，", "你好，团子"]);
}

#[test]
fn chunked_http_and_sse_boundaries_can_split_chinese_and_done_at_every_byte() {
    let payload = sse_payload(&responses_events());
    for chunk_size in [1, 3, 37, 1024] {
        for tcp_size in [1, 7, 2048] {
            let mut wire=b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            for chunk in payload.chunks(chunk_size) {
                wire.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                wire.extend_from_slice(chunk);
                wire.extend_from_slice(b"\r\n");
            }
            wire.extend_from_slice(b"0\r\nX-Trace: offline\r\n\r\n");
            run(async {
                let mut http = Response::new(Fragments(&wire, tcp_size));
                let head = http.head().await.unwrap();
                assert_eq!(head.status, 200);
                assert_eq!(
                    head.content_type.as_deref(),
                    Some("text/event-stream; charset=utf-8")
                );
                let mut sse = sse::Decoder::default();
                let mut reply = Reply::default();
                let mut visible = Vec::new();
                let mut out = [0; 19];
                loop {
                    let n = http.read(&mut out).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    for b in &out[..n] {
                        if let Some(data) = sse.push(*b).unwrap() {
                            if reply.event(data).unwrap() {
                                visible.push(reply.text.clone());
                            }
                        }
                    }
                }
                assert!(reply.done);
                assert_eq!(reply.text, "你好，团子");
                assert_eq!(visible, ["你好", "你好，", "你好，团子"]);
            });
        }
    }
}

#[test]
fn premature_eof_after_text_and_item_done_is_not_a_completed_reply() {
    let mut events = responses_events();
    events.pop();
    let payload = sse_payload(&events);
    let mut wire = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
        payload.len()
    )
    .into_bytes();
    wire.extend_from_slice(&payload);
    run(async {
        let mut http = Response::new(Fragments(&wire, 7));
        http.head().await.unwrap();
        let mut sse = sse::Decoder::default();
        let mut reply = Reply::default();
        let mut bytes = [0; 19];
        loop {
            let n = http.read(&mut bytes).await.unwrap();
            if n == 0 {
                break;
            }
            for byte in &bytes[..n] {
                if let Some(data) = sse.push(*byte).unwrap() {
                    reply.event(data).unwrap();
                }
            }
        }
        assert_eq!(reply.text, "你好，团子");
        assert!(!reply.done);
    });
}

#[test]
fn recorded_deepseek_responses_stream_completes_without_a_done_sentinel() {
    let mut decoder = sse::Decoder::default();
    let mut reply = Reply::default();
    let mut visible = Vec::new();
    for byte in include_bytes!("fixtures/deepseek-responses.sse") {
        if let Some(data) = decoder.push(*byte).unwrap() {
            if reply.event(data).unwrap() {
                visible.push(reply.text.clone());
            }
        }
    }
    assert!(reply.done);
    assert_eq!(reply.text, "团子已连接。");
    assert_eq!(
        visible,
        ["团", "团子", "团子已", "团子已连接", "团子已连接。"]
    );
}
