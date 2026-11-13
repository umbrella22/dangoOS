#[cfg(not(target_os = "none"))]
use crate::json_stream::TextSource;
use crate::json_stream::{Event, Json, TextSpan};
#[cfg(test)]
use alloc::format;
use alloc::{string::String, vec::Vec};
struct Part {
    item: String,
    output: u64,
    content: u64,
    refusal: bool,
    closed: bool,
    start: TextSpan,
}
#[derive(Default)]
pub struct Reply {
    #[cfg(not(target_os = "none"))]
    pub text: String,
    pub done: bool,
    response: Option<String>,
    sequence: Option<u64>,
    part: Option<Part>,
    span: TextSpan,
    parts: Vec<TextSpan>,
    base: TextSpan,
    pub function_calls: usize,
}
impl Reply {
    pub fn json(&self) -> Json {
        Json::new(
            self.span.clone(),
            self.part.as_ref().map_or_else(TextSpan::default, |p| {
                if p.closed {
                    self.span.clone()
                } else {
                    p.start.clone()
                }
            }),
        )
        .snapshot_base(self.base.clone())
    }
    pub fn next_round(&mut self) -> Result<(), &'static str> {
        if !self.done || self.function_calls == 0 {
            return Err("AI 工具轮次尚未结束");
        }
        let span = self.span.clone();
        *self = Self {
            #[cfg(not(target_os = "none"))]
            text: core::mem::take(&mut self.text),
            base: span.clone(),
            span,
            ..Self::default()
        };
        Ok(())
    }
    pub fn bytes(&self) -> usize {
        self.span.bytes
    }
    #[cfg(not(target_os = "none"))]
    pub fn event(&mut self, data: &[u8]) -> Result<bool, &'static str> {
        let mut json = self.json();
        let mut delta = String::new();
        for b in data {
            json.push(*b, &mut |source, bytes| {
                Ok(match source {
                    TextSource::Append => {
                        delta.push_str(core::str::from_utf8(bytes).unwrap());
                        true
                    }
                    TextSource::Compare(offset) => {
                        self.text
                            .as_bytes()
                            .get(offset..offset.saturating_add(bytes.len()))
                            == Some(bytes)
                    }
                    TextSource::OutputStart | TextSource::Output => true,
                })
            })?;
        }
        let changed = self.accept(json.finish()?)?;
        if changed {
            self.text.push_str(&delta);
        }
        Ok(changed)
    }
    #[inline(never)]
    pub fn accept(&mut self, e: Event) -> Result<bool, &'static str> {
        if self.done {
            return Err("AI 流结束后仍收到数据");
        }
        match e.kind.as_str() {
            "error" | "response.failed" => return Err("AI 服务返回错误，请检查服务配置"),
            "response.incomplete" => {
                return Err(match e.reason.as_str() {
                    "max_output_tokens" => "AI 回复达到令牌上限，内容未完成",
                    "content_filter" => "AI 服务中止了回复",
                    _ => "AI 回复未完成",
                })
            }
            _ => (),
        }
        if e.kind.is_empty() {
            return Err("AI 流缺少事件类型");
        }
        let sequence = e.sequence.ok_or("AI 流事件序号无效")?;
        if self.sequence.is_some_and(|previous| sequence <= previous) {
            return Err("AI 流事件顺序无效");
        }
        self.sequence = Some(sequence);
        if e.kind == "response.created" {
            if self.response.is_some() || e.response.is_empty() {
                return Err("AI 流事件顺序无效");
            }
            self.response = Some(e.response);
            return Ok(false);
        }
        if self.response.is_none() {
            return Err("AI 流事件顺序无效");
        }
        match e.kind.as_str() {
            "response.output_text.delta" => return self.text_event(e, false, false),
            "response.refusal.delta" => return self.text_event(e, true, false),
            "response.output_text.done" => return self.text_event(e, false, true),
            "response.refusal.done" => return self.text_event(e, true, true),
            "response.output_item.added" | "response.output_item.done" => {
                if !e.item_seen || e.unsupported {
                    return Err("当前语音会话只支持文字回复");
                }
            }
            "response.completed" => {
                if self.response.as_deref() != Some(e.response.as_str())
                    || !e.completed
                    || e.error
                    || e.incomplete
                    || self.part.as_ref().is_some_and(|p| !p.closed)
                    || (e.function_calls == 0
                        && (self.span.bytes == self.base.bytes || !self.span.visible))
                {
                    return Err("AI 回复不完整或为空");
                }
                if e.unsupported {
                    return Err("当前语音会话只支持文字回复");
                }
                if e.bad_item {
                    return Err("AI 回复未完成");
                }
                if !e.snapshot_seen
                    || e.snapshot.len() != self.parts.len()
                    || !e.snapshot.iter().zip(&self.parts).all(|(a, b)| a.same(b))
                {
                    return Err("AI 最终回复与增量不匹配");
                }
                self.done = true;
                self.function_calls = e.function_calls;
            }
            _ => (),
        }
        Ok(false)
    }
    #[inline(never)]
    fn text_event(
        &mut self,
        e: Event,
        refusal: bool,
        final_text: bool,
    ) -> Result<bool, &'static str> {
        if e.item.is_empty() {
            return Err("AI 流条目标识无效");
        }
        let output = e.output.ok_or("AI 流事件顺序无效")?;
        let content = e.content.ok_or("AI 流事件顺序无效")?;
        let same = self
            .part
            .as_ref()
            .is_some_and(|p| p.item == e.item && p.output == output && p.content == content);
        if !same {
            if self.part.as_ref().is_some_and(|p| {
                !p.closed
                    || if p.item == e.item {
                        output != p.output || content <= p.content
                    } else {
                        output <= p.output
                    }
            }) {
                return Err("AI 流事件顺序无效");
            }
            self.part = Some(Part {
                item: e.item,
                output,
                content,
                refusal,
                closed: false,
                start: self.span.clone(),
            });
        }
        let part = self.part.as_mut().unwrap();
        if part.closed || part.refusal != refusal {
            return Err("AI 流事件顺序无效");
        }
        let span = e.text.ok_or("AI 流文字无效")?;
        if !final_text {
            let changed = span.bytes != self.span.bytes;
            self.span = span;
            return Ok(changed);
        }
        if !span.same(&self.span) {
            return Err("AI 最终回复与增量不匹配");
        }
        part.closed = true;
        self.parts.try_reserve(1).map_err(|_| "AI 解析内存不足")?;
        self.parts.push(span);
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use serde_json::{json, Value};

    struct Stream {
        reply: Reply,
        sequence: u64,
    }
    impl Stream {
        fn event(&mut self, mut value: Value) -> Result<bool, &'static str> {
            value["sequence_number"] = self.sequence.into();
            self.sequence += 1;
            self.reply.event(&serde_json::to_vec(&value).unwrap())
        }
        fn new() -> Self {
            let mut s = Self {
                reply: Reply::default(),
                sequence: 0,
            };
            s.event(
                json!({"type":"response.created","response":{"id":"r","status":"in_progress"}}),
            )
            .unwrap();
            s
        }
        fn message(&mut self) {
            self.event(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"m","type":"message","role":"assistant","content":[]}})).unwrap();
        }
        fn delta(&mut self, text: &str) -> Result<bool, &'static str> {
            self.event(json!({"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":text}))
        }
        fn text_done(&mut self, text: &str) {
            self.event(json!({"type":"response.output_text.done","item_id":"m","output_index":0,"content_index":0,"text":text})).unwrap();
        }
        fn streamed(text: &str) -> Self {
            let mut s = Self::new();
            s.message();
            s.delta(text).unwrap();
            s.text_done(text);
            s.event(
                json!({"type":"response.output_item.done","output_index":0,"item":message(text)}),
            )
            .unwrap();
            s
        }
    }
    fn message(text: &str) -> Value {
        json!({"id":"m","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":text}]})
    }
    fn completed(text: &str) -> Value {
        json!({"type":"response.completed","response":{"id":"r","status":"completed","error":null,"incomplete_details":null,"output":[message(text)]}})
    }

    #[test]
    fn text_done_is_not_response_completion_and_terminal_snapshot_does_not_duplicate_text() {
        let mut s = Stream::streamed("你好");
        assert_eq!(s.reply.text, "你好");
        assert!(!s.reply.done);
        assert!(!s.event(completed("你好")).unwrap());
        assert!(s.reply.done);
        assert_eq!(s.reply.text, "你好");
        assert!(s.event(completed("你好")).is_err());
        assert!(Reply::default().event(b"[DONE]").is_err());
    }

    #[test]
    fn refusal_is_streamed_and_finalized_as_visible_text() {
        let mut s = Stream::new();
        s.message();
        assert!(s.event(json!({"type":"response.refusal.delta","item_id":"m","output_index":0,"content_index":0,"delta":"无法回答"})).unwrap());
        s.event(json!({"type":"response.refusal.done","item_id":"m","output_index":0,"content_index":0,"refusal":"无法回答"})).unwrap();
        let item = json!({"id":"m","type":"message","role":"assistant","status":"completed","content":[{"type":"refusal","refusal":"无法回答"}]});
        s.event(json!({"type":"response.output_item.done","output_index":0,"item":item}))
            .unwrap();
        let mut end = completed("");
        end["response"]["output"] = json!([item]);
        s.event(end).unwrap();
        assert!(s.reply.done);
        assert_eq!(s.reply.text, "无法回答");
    }

    #[test]
    fn sequential_assistant_messages_verify_all_text_parts() {
        let mut s = Stream::streamed("first");
        let item = json!({"id":"n","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"second"}]});
        s.event(json!({"type":"response.output_item.added","output_index":1,"item":{"id":"n","type":"message","role":"assistant","content":[]}})).unwrap();
        s.event(json!({"type":"response.output_text.delta","item_id":"n","output_index":1,"content_index":0,"delta":"second"})).unwrap();
        s.event(json!({"type":"response.output_text.done","item_id":"n","output_index":1,"content_index":0,"text":"second"})).unwrap();
        s.event(json!({"type":"response.output_item.done","output_index":1,"item":item}))
            .unwrap();
        let mut end = completed("first");
        end["response"]["output"].as_array_mut().unwrap().push(item);
        s.event(end).unwrap();
        assert!(s.reply.done);
        assert_eq!(s.reply.text, "firstsecond");
    }

    #[test]
    fn failed_incomplete_and_error_preserve_partial_text_without_exposing_provider_errors() {
        for event in [
            json!({"type":"response.failed","response":{"error":{"message":"private-provider-detail"}}}),
            json!({"type":"error","message":"private-provider-detail"}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"unknown"}}}),
        ] {
            let mut s = Stream::new();
            s.message();
            s.delta("部分回复").unwrap();
            let error = s.event(event).unwrap_err();
            assert!(!error.contains("private-provider-detail"));
            assert!(!s.reply.done);
            assert_eq!(s.reply.text, "部分回复");
        }
    }

    #[test]
    fn completed_requires_matching_session_status_and_final_output() {
        for (field, value) in [
            ("id", json!("other-response")),
            ("status", json!("incomplete")),
            ("error", json!({"code":"server_error"})),
            ("error", json!("server error")),
            ("error", json!(0)),
            ("incomplete_details", json!(true)),
            ("incomplete_details", json!({"reason":"max_output_tokens"})),
            ("output", json!([message("changed-final-text")])),
            ("output", json!([])),
            ("output", json!([{"type":"function_call"}])),
        ] {
            let mut s = Stream::streamed("partial");
            let mut end = completed("partial");
            end["response"][field] = value;
            assert!(s.event(end).is_err());
            assert!(!s.reply.done);
            assert_eq!(s.reply.text, "partial");
        }
        let mut s = Stream::new();
        s.message();
        s.delta("partial").unwrap();
        assert!(s.event(completed("partial")).is_err());
        let mut empty = Stream::streamed("   ");
        assert!(empty.event(completed("   ")).is_err());
    }

    #[test]
    fn deltas_must_match_item_part_and_monotonic_sequence_and_cannot_follow_text_done() {
        for (field, value) in [
            ("item_id", json!("other-item")),
            ("output_index", json!(1)),
            ("content_index", json!(1)),
            ("type", json!("response.refusal.delta")),
        ] {
            let mut s = Stream::new();
            s.message();
            s.delta("first").unwrap();
            let mut delta = json!({"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":"second"});
            delta[field] = value;
            assert!(s.event(delta).is_err());
            assert_eq!(s.reply.text, "first");
        }
        let mut s = Stream::new();
        s.message();
        let event = json!({"type":"response.output_text.delta","sequence_number":2,"item_id":"m","output_index":0,"content_index":0,"delta":"first"}).to_string();
        s.reply.event(event.as_bytes()).unwrap();
        assert!(s.reply.event(event.as_bytes()).is_err());
        s.sequence = 3;
        s.text_done("first");
        assert!(s.delta("late").is_err());
        assert_eq!(s.reply.text, "first");
        let mut missing = Reply::default();
        assert!(missing.event(br#"{"type":"response.completed","sequence_number":1,"response":{"id":"r","status":"completed","output":[]}}"#).is_err());
    }

    #[test]
    fn long_text_is_accepted_and_nul_is_rejected_without_truncation() {
        let text = "你".repeat(30_000);
        let mut s = Stream::new();
        s.message();
        s.delta(&text).unwrap();
        s.delta("🙂").unwrap();
        assert_eq!(s.reply.text, format!("{text}🙂"));
        let mut s = Stream::new();
        s.message();
        s.delta("visible").unwrap();
        assert!(s.delta("\0hidden").is_err());
        assert_eq!(s.reply.text, "visible");
    }

    #[test]
    fn unsupported_tools_are_rejected_and_function_calls_require_a_completed_round() {
        let mut s = Stream::new();
        let call = json!({"id":"t","type":"function_call","status":"completed","call_id":"c","name":"device_status","arguments":"{}"});
        s.event(json!({"type":"response.output_item.added","output_index":0,"item":call}))
            .unwrap();
        assert!(!s.reply.done);
        s.event(json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[call]}})).unwrap();
        assert!(s.reply.done);
        assert_eq!(s.reply.function_calls, 1);
        for kind in ["web_search_call", "image_generation_call"] {
            let mut s = Stream::new();
            assert!(s.event(json!({"type":"response.output_item.added","output_index":0,"item":{"id":"t","type":kind}})).is_err());
            assert!(s.reply.text.is_empty());
        }
        let mut s = Stream::new();
        s.message();
        s.delta("partial").unwrap();
        assert!(s.event(json!({"type":"response.output_text.done","item_id":"m","output_index":0,"content_index":0,"text":"different"})).is_err());
        assert_eq!(s.reply.text, "partial");
        // Constructing final text from snapshots would hide a lost delta.
        let mut s = Stream::new();
        s.message();
        s.event(json!({"type":"response.output_item.done","output_index":0,"item":message("missing-deltas")})).unwrap();
        assert!(s.event(completed("missing-deltas")).is_err());
        assert_eq!(s.reply.text, "");
    }
}
