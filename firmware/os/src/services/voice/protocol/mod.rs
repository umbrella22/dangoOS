//! OpenAI-compatible Realtime transcription and Responses contracts.
use super::{append_bounded, json_field, json_string, TRANSCRIPT_BYTES};
use alloc::{format, string::String};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;

pub mod responses;
pub use responses::Reply;

pub fn session_update(model: &str) -> String {
    let model = json_string(model);
    format!(
        concat!(
            "{{\"type\":\"session.update\",\"session\":{{\"type\":\"transcription\",",
            "\"audio\":{{\"input\":{{\"format\":{{\"type\":\"audio/pcm\",\"rate\":24000}},",
            "\"transcription\":{{\"model\":{}}},\"turn_detection\":null}}}}}}}}"
        ),
        model
    )
}
pub fn append_audio(pcm: &[u8], out: &mut String) {
    out.clear();
    out.push_str("{\"type\":\"input_audio_buffer.append\",\"audio\":\"");
    STANDARD.encode_string(pcm, out);
    out.push_str("\"}");
}
pub const COMMIT: &str = "{\"type\":\"input_audio_buffer.commit\"}";

#[derive(Default)]
pub struct Transcript {
    pub updated: bool,
    pub text: String,
    pub completed: bool,
    item: Option<String>,
    committed: Option<String>,
    pub sent_commit: bool,
}
impl Transcript {
    pub fn event(&mut self, bytes: &[u8]) -> Result<bool, &'static str> {
        let v: Value = serde_json::from_slice(bytes).map_err(|_| "转写服务 JSON 无效")?;
        let kind = json_field(&v, "type").as_str().ok_or("转写服务事件无效")?;
        match kind {
            "error" | "conversation.item.input_audio_transcription.failed" => {
                return Err("转写服务返回错误，请检查服务配置")
            }
            "session.updated" => self.updated = true,
            "input_audio_buffer.committed" => {
                if !self.sent_commit || self.committed.is_some() {
                    return Err("转写提交顺序无效");
                }
                let item = item_id(&v)?;
                if self.item.as_deref().is_some_and(|id| id != item) {
                    return Err("转写会话不匹配");
                }
                self.committed = Some(item.into());
            }
            "conversation.item.input_audio_transcription.delta" => {
                let item = item_id(&v)?;
                self.accept_item(item)?;
                let delta = json_field(&v, "delta").as_str().ok_or("转写增量文字无效")?;
                append_bounded(&mut self.text, delta, TRANSCRIPT_BYTES)?;
                return Ok(true);
            }
            "conversation.item.input_audio_transcription.completed" => {
                let item = item_id(&v)?;
                if self.committed.as_deref() != Some(item) {
                    return Err("转写结果未对应已提交录音");
                }
                self.accept_item(item)?;
                let text = json_field(&v, "transcript")
                    .as_str()
                    .ok_or("转写最终文字无效")?;
                if text.trim().is_empty() {
                    return Err("没有识别到有效语音");
                }
                let mut final_text = String::new();
                append_bounded(&mut final_text, text, TRANSCRIPT_BYTES)?;
                self.text = final_text;
                self.completed = true;
                return Ok(true);
            }
            _ => (),
        }
        Ok(false)
    }
    fn accept_item(&mut self, item: &str) -> Result<(), &'static str> {
        if self.completed
            || self.item.as_deref().is_some_and(|id| id != item)
            || self.committed.as_deref().is_some_and(|id| id != item)
        {
            return Err("转写会话不匹配");
        }
        if self.item.is_none() {
            self.item = Some(item.into());
        }
        Ok(())
    }
}
fn item_id(v: &Value) -> Result<&str, &'static str> {
    json_field(v, "item_id")
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("转写条目标识无效")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn committed_item_is_required_and_final_text_replaces_partial() {
        let mut t = Transcript::default();
        t.event(br#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"a","delta":"hi"}"#).unwrap();
        t.sent_commit = true;
        t.event(br#"{"type":"input_audio_buffer.committed","item_id":"a"}"#)
            .unwrap();
        t.event(br#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"hello"}"#).unwrap();
        assert_eq!(t.text, "hello");
        assert!(t.completed);
        assert!(Transcript::default().event(br#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"old"}"#).is_err());
    }
    #[test]
    fn audio_is_pcm24k_and_base64_is_not_mislabeled_wav() {
        let v: Value = serde_json::from_str(&session_update("configured-model")).unwrap();
        assert_eq!(v["session"]["audio"]["input"]["format"]["rate"], 24000);
        assert_eq!(
            v["session"]["audio"]["input"]["turn_detection"],
            Value::Null
        );
        let mut s = String::new();
        append_audio(&[0, 1, 2, 3], &mut s);
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["audio"], "AAECAw==");
    }
    #[test]
    fn session_model_preserves_quotes_slashes_and_unicode() {
        let model = "voice\"\\模型";
        let session: Value = serde_json::from_str(&session_update(model)).unwrap();
        assert_eq!(
            session["session"]["audio"]["input"]["transcription"]["model"],
            model
        );
    }
}
