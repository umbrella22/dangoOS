//! Separately provisioned credentials; never included in status or Debug output.
use super::{json_field, json_get};
use crate::formats::http::{HttpProxy, Url};
use alloc::{format, string::String};

pub const PATH: &str = "/data/settings/voice.json";
pub const STAGING: &str = "/data/settings/.voice-incoming";
pub const STAGED: &str = "/data/settings/.voice-incoming/voice/config.json";
pub const MAX_BYTES: usize = 8192;

pub struct Endpoint {
    pub url: Url,
    pub key: String,
    pub model: String,
}
pub struct Config {
    pub asr: Endpoint,
    // The text conversation service uses the Responses protocol.
    pub chat: Endpoint,
    pub proxy: Option<HttpProxy>,
    pub prompt: String,
}
impl Config {
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_BYTES {
            return Err("语音配置过大");
        }
        let v: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "语音配置 JSON 无效")?;
        if json_field(&v, "schema").as_u64() != Some(1) {
            return Err("语音配置版本无效");
        }
        let endpoint = |v: &serde_json::Value, websocket: bool| -> Result<Endpoint, &'static str> {
            let raw = json_field(v, "url").as_str().ok_or("缺少语音服务地址")?;
            if raw.len() > 1024 {
                return Err("语音服务地址过长");
            }
            let raw = if websocket {
                format!(
                    "https://{}",
                    raw.strip_prefix("wss://").ok_or("转写服务需要 wss 地址")?
                )
            } else {
                String::from(raw)
            };
            let url = Url::parse(&raw).map_err(|_| "语音服务需要有效的加密地址")?;
            let key = json_field(v, "api_key")
                .as_str()
                .ok_or("缺少语音服务密钥")?;
            if key.is_empty() || key.len() > 512 || !key.bytes().all(|b| (33..=126).contains(&b)) {
                return Err("语音服务密钥无效");
            }
            let model = json_field(v, "model").as_str().ok_or("缺少语音服务模型")?;
            if model.is_empty() || model.len() > 128 || model.chars().any(char::is_control) {
                return Err("语音服务模型无效");
            }
            Ok(Endpoint {
                url,
                key: key.into(),
                model: model.into(),
            })
        };
        let proxy = match json_get(&v, "proxy") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(
                HttpProxy::parse(v.as_str().ok_or("语音代理需要 IPv4:端口")?)
                    .map_err(|_| "语音代理需要 IPv4:端口")?,
            ),
        };
        let prompt = match json_get(&v, "system_prompt") {
            None => "你是桌面伙伴团子。请用简短、自然的中文回答。".into(),
            Some(v) => {
                let text = v.as_str().ok_or("语音提示词需要文字")?;
                if text.len() > 1024 || text.contains('\0') {
                    return Err("语音提示词无效");
                }
                text.into()
            }
        };
        Ok(Self {
            asr: endpoint(json_field(&v, "asr"), true)?,
            chat: endpoint(json_field(&v, "chat"), false)?,
            proxy,
            prompt,
        })
    }
}

pub fn load(vfs: &mut fs::Vfs) -> Result<Config, &'static str> {
    if vfs.stat(PATH).map_err(|_| "请先配置语音服务")?.size as usize > MAX_BYTES {
        return Err("语音配置过大");
    }
    Config::parse(&vfs.read_file(PATH).map_err(|_| "无法读取语音配置")?)
}

pub fn commit(vfs: &mut fs::Vfs) -> Result<(), &'static str> {
    if vfs.stat(STAGED).map_err(|_| "缺少语音配置文件")?.size as usize > MAX_BYTES {
        return Err("语音配置过大");
    }
    Config::parse(&vfs.read_file(STAGED).map_err(|_| "无法读取语音配置")?)?;
    vfs.rename(STAGED, PATH).map_err(|_| "无法保存语音配置")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value() -> serde_json::Value {
        serde_json::json!({"schema":1,"asr":{"url":"wss://asr.example/v1/realtime?intent=transcription","api_key":"secret-asr","model":"transcribe"},
            "chat":{"url":"https://chat.example/gateway/v1/responses?route=voice","api_key":"secret-chat","model":"chat"},"proxy":"192.0.2.10:7890"})
    }
    #[test]
    fn separate_providers_preserve_paths_and_reject_header_injection_and_cleartext() {
        let good = value();
        let config = Config::parse(&serde_json::to_vec(&good).unwrap()).unwrap();
        assert_eq!(config.asr.url.host, "asr.example");
        assert_eq!(config.chat.url.host, "chat.example");
        assert_eq!(config.chat.url.path, "/gateway/v1/responses?route=voice");
        assert_eq!(config.asr.url.path, "/v1/realtime?intent=transcription");
        assert_eq!(config.proxy.unwrap().port, 7890);
        for (endpoint, field, bad) in [
            ("asr", "api_key", "key\r\nInjected: yes"),
            ("asr", "url", "ws://asr.example"),
            ("chat", "url", "http://chat.example"),
            ("chat", "url", "https://key@chat.example"),
            ("chat", "model", ""),
        ] {
            let mut v = value();
            v[endpoint][field] = bad.into();
            let e = Config::parse(&serde_json::to_vec(&v).unwrap())
                .err()
                .unwrap();
            assert!(!e.contains("secret"));
        }
    }
}
