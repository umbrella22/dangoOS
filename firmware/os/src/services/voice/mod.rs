//! One button-defined voice turn: bounded capture, realtime transcription,
//! then a streamed text answer. Audio and UI keep their existing sole owners.
use alloc::string::String;

pub mod capture;
pub mod config;
pub mod desktop;
#[cfg(target_os = "none")]
pub(crate) mod device;
#[cfg(target_os = "none")]
mod network;
pub mod protocol;
pub mod sse;
pub mod storage;
pub mod websocket;

pub const TRANSCRIPT_BYTES: usize = 1024;
pub use voice_session::WINDOW_BYTES;
pub const MAX_RECORDING_MS: u64 = 30_000;
pub const MIN_RECORDING_MS: u64 = 300;

// Share object lookup across constant keys; LTO otherwise duplicates the map
// walk throughout the configuration and event parsers.
#[inline(never)]
fn json_field<'a>(value: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    json_get(value, key).unwrap_or(&serde_json::Value::Null)
}

#[inline(never)]
fn json_get<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    value.get(key)
}

#[inline(never)]
fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("string serialization is infallible")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Preparing,
    Listening,
    Transcribing,
    Replying,
    Done,
    Cancelled,
    Error,
}
impl Phase {
    pub const fn active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Listening | Self::Transcribing | Self::Replying
        )
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Preparing => "preparing",
            Self::Listening => "listening",
            Self::Transcribing => "transcribing",
            Self::Replying => "replying",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub session: u32,
    pub phase: Phase,
    pub recorded_ms: u32,
    pub transcript: String,
    pub reply: String,
    pub reply_start: usize,
    pub reply_bytes: usize,
    pub reply_path: String,
    pub error: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Cancel,
    Status,
    Reload,
    Ask(String),
    Read(usize),
    Page(usize),
    Desktop(desktop::Command),
}
impl Command {
    pub fn parse(verb: &str) -> Option<Self> {
        Some(match verb {
            "voice-toggle" => Self::Toggle,
            "voice-cancel" => Self::Cancel,
            "voice-status" => Self::Status,
            "voice-reload" => Self::Reload,
            _ => return None,
        })
    }

    #[inline(never)]
    pub fn ask(encoded: &str) -> Result<Self, &'static str> {
        let bytes = crate::formats::hex::decode(encoded, TRANSCRIPT_BYTES)
            .map_err(|_| "invalid voice text encoding or length")?;
        let text = String::from_utf8(bytes).map_err(|_| "invalid voice text UTF-8")?;
        if text.trim().is_empty() || text.contains('\0') {
            return Err("voice text is empty or contains NUL");
        }
        Ok(Self::Ask(text))
    }
}

pub fn reply_record(session: u32, offset: usize, total: usize, hex: &str) -> String {
    alloc::format!("@ok voice-read session={session} offset={offset} total={total} hex={hex}")
}

pub fn append_bounded(target: &mut String, text: &str, limit: usize) -> Result<(), &'static str> {
    if target.len().saturating_add(text.len()) > limit {
        return Err("语音文字超过长度上限");
    }
    // LVGL accepts NUL-terminated text. Reject an unrepresentable reply at the
    // protocol boundary rather than silently showing a truncated conversation.
    if text.contains('\0') {
        return Err("语音服务返回了无效文字");
    }
    target.push_str(text);
    Ok(())
}
