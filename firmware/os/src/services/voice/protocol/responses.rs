//! Provider request encoding; streamed parsing is shared with NAND storage.
use super::super::{config::Config, json_string};
use alloc::{format, string::String};
pub use voice_session::Reply;
pub fn prefix(config: &Config) -> String {
    format!(
        "{{\"model\":{},\"instructions\":{},\"input\":[",
        json_string(&config.chat.model),
        json_string(&config.prompt)
    )
}
pub fn suffix() -> String {
    format!(
        "],\"stream\":true,\"store\":false,\"tools\":{}}}",
        voice_session::agent::TOOLS
    )
}
pub fn head(config: &Config, body_bytes: usize) -> String {
    format!("POST {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nAccept-Encoding: identity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        config.chat.url.path,config.chat.url.authority(),config.chat.key,body_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[test]
    fn request_preserves_text_and_provider_path_without_a_client_token_cap() {
        let model = "voice\"\\模型";
        let config = Config::parse(&serde_json::to_vec(&json!({
            "schema":1,
            "asr":{"url":"wss://asr.example/realtime","api_key":"asr-key","model":"asr"},
            "chat":{"url":"https://chat.example:8443/gateway/v1/responses?route=voice","api_key":"chat-key","model":model},
            "system_prompt":"quote: \" slash: \\ newline:\n"
        })).unwrap()).unwrap();
        let text = "第一行\n\"第二行\"\\🙂";
        let context = json!({"type":"message","role":"user","content":text}).to_string();
        let body = format!("{}{}{}", prefix(&config), context, suffix());
        let request = format!("{}{}", head(&config, body.len()), body);
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with(
            "POST /gateway/v1/responses?route=voice HTTP/1.1\r\nHost: chat.example:8443\r\n"
        ));
        assert!(headers.contains("Authorization: Bearer chat-key\r\n"));
        assert!(!headers.contains("asr-key"));
        assert!(headers.contains(&format!("Content-Length: {}\r\n", body.len())));
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            body,
            json!({"model":model,"instructions":config.prompt,"input":[{"type":"message","role":"user","content":text}],"stream":true,"store":false,"tools":serde_json::from_str::<Value>(voice_session::agent::TOOLS).unwrap()})
        );
    }
}
