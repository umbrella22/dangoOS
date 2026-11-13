//! Bounded RFC 6455 framing, independent of TCP/TLS packet boundaries.
use super::config::Endpoint;
use alloc::{format, string::String, vec::Vec};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha1::{Digest, Sha1};

pub const MESSAGE_BYTES: usize = 16 * 1024;
pub fn request(endpoint: &Endpoint, nonce: &[u8; 16]) -> String {
    format!("GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        endpoint.url.path, endpoint.url.authority(), endpoint.key, STANDARD.encode(nonce))
}
pub struct Upgrade {
    bytes: Vec<u8>,
    accept: String,
}
impl Upgrade {
    pub fn new(nonce: &[u8; 16]) -> Self {
        let mut hash = Sha1::new();
        hash.update(STANDARD.encode(nonce));
        hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        Self {
            bytes: Vec::new(),
            accept: STANDARD.encode(hash.finalize()),
        }
    }
    pub fn push(&mut self, byte: u8) -> Result<bool, &'static str> {
        if self.bytes.len() == 8192 {
            return Err("语音握手响应过大");
        }
        self.bytes.push(byte);
        if !self.bytes.ends_with(b"\r\n\r\n") {
            return Ok(false);
        }
        let head = core::str::from_utf8(&self.bytes).map_err(|_| "语音握手响应无效")?;
        let mut lines = head.split("\r\n");
        let mut status = lines.next().unwrap_or("").splitn(3, ' ');
        if status.next() != Some("HTTP/1.1") {
            return Err("语音握手协议无效");
        }
        match status.next() {
            Some("101") => (),
            Some("401" | "403") => return Err("转写服务认证失败"),
            Some("429") => return Err("转写服务额度或速率受限"),
            _ => return Err("转写服务不支持实时连接"),
        }
        let (mut upgrade, mut connection, mut accept) = (false, false, None);
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':').ok_or("语音握手响应无效")?;
            let value = value.trim();
            if name.eq_ignore_ascii_case("upgrade") {
                if upgrade || !value.eq_ignore_ascii_case("websocket") {
                    return Err("语音 Upgrade 无效");
                }
                upgrade = true;
            } else if name.eq_ignore_ascii_case("connection") {
                connection |= value
                    .split(',')
                    .any(|v| v.trim().eq_ignore_ascii_case("upgrade"));
            } else if name.eq_ignore_ascii_case("sec-websocket-accept") {
                if accept.replace(value).is_some() {
                    return Err("语音握手签名重复");
                }
            } else if name.eq_ignore_ascii_case("sec-websocket-extensions")
                || name.eq_ignore_ascii_case("sec-websocket-protocol")
            {
                return Err("语音服务启用了未协商的扩展");
            }
        }
        if !upgrade || !connection || accept != Some(self.accept.as_str()) {
            return Err("语音握手校验失败");
        }
        Ok(true)
    }
}

pub fn encode(
    opcode: u8,
    payload: &[u8],
    mask: [u8; 4],
    out: &mut Vec<u8>,
) -> Result<(), &'static str> {
    if !matches!(opcode, 1 | 8 | 9 | 10)
        || payload.len() > MESSAGE_BYTES
        || (opcode >= 8 && payload.len() > 125)
    {
        return Err("语音发送帧无效");
    }
    out.clear();
    out.push(0x80 | opcode);
    if payload.len() < 126 {
        out.push(0x80 | payload.len() as u8);
    } else {
        out.push(0x80 | 126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event<'a> {
    Text(&'a str),
    Ping(&'a [u8]),
    Pong,
    Close,
}
pub struct Decoder {
    header: [u8; 10],
    used: usize,
    need: usize,
    remaining: usize,
    opcode: u8,
    fin: bool,
    fragmented: bool,
    text: Vec<u8>,
    control: Vec<u8>,
    clear_text: bool,
}
impl Default for Decoder {
    fn default() -> Self {
        Self {
            header: [0; 10],
            used: 0,
            need: 2,
            remaining: 0,
            opcode: 0,
            fin: false,
            fragmented: false,
            text: Vec::new(),
            control: Vec::new(),
            clear_text: false,
        }
    }
}
impl Decoder {
    pub fn push(&mut self, byte: u8) -> Result<Option<Event<'_>>, &'static str> {
        if self.clear_text {
            self.text.clear();
            self.clear_text = false;
        }
        if self.used < self.need {
            self.header[self.used] = byte;
            self.used += 1;
            if self.used == 2 {
                if self.header[0] & 0x70 != 0 || byte & 0x80 != 0 {
                    return Err("语音帧扩展或掩码无效");
                }
                self.need = match byte & 0x7f {
                    126 => 4,
                    127 => 10,
                    _ => 2,
                };
            }
            if self.used < self.need {
                return Ok(None);
            }
            self.opcode = self.header[0] & 15;
            self.fin = self.header[0] & 128 != 0;
            let len = match self.need {
                2 => u64::from(self.header[1] & 127),
                4 => {
                    let n = u16::from_be_bytes([self.header[2], self.header[3]]) as u64;
                    if n < 126 {
                        return Err("语音帧长度编码无效");
                    }
                    n
                }
                _ => {
                    let n = u64::from_be_bytes(self.header[2..10].try_into().unwrap());
                    if n < 65536 {
                        return Err("语音帧长度编码无效");
                    }
                    n
                }
            };
            if len > MESSAGE_BYTES as u64 {
                return Err("语音帧超过长度上限");
            }
            if self.opcode >= 8 {
                if !self.fin || len > 125 || !matches!(self.opcode, 8..=10) {
                    return Err("语音控制帧无效");
                }
                self.control.clear();
            } else {
                if !matches!(self.opcode, 0 | 1) || (self.opcode == 0) != self.fragmented {
                    return Err("语音分片顺序无效");
                }
                if self.text.len() + len as usize > MESSAGE_BYTES {
                    return Err("语音消息超过长度上限");
                }
            }
            self.remaining = len as usize;
            if self.remaining != 0 {
                return Ok(None);
            }
        } else {
            if self.opcode >= 8 {
                self.control.push(byte);
            } else {
                self.text.push(byte);
            }
            self.remaining -= 1;
            if self.remaining != 0 {
                return Ok(None);
            }
        }
        self.used = 0;
        self.need = 2;
        Ok(match self.opcode {
            0 | 1 if self.fin => {
                self.fragmented = false;
                self.clear_text = true;
                Some(Event::Text(
                    core::str::from_utf8(&self.text).map_err(|_| "语音文字编码无效")?,
                ))
            }
            0 | 1 => {
                self.fragmented = true;
                None
            }
            8 => {
                if self.control.len() == 1 {
                    return Err("语音关闭帧无效");
                }
                if self.control.len() >= 2 {
                    let code = u16::from_be_bytes([self.control[0], self.control[1]]);
                    if !matches!(code,1000..=1003|1007..=1014|3000..=4999)
                        || core::str::from_utf8(&self.control[2..]).is_err()
                    {
                        return Err("语音关闭帧无效");
                    }
                }
                Some(Event::Close)
            }
            9 => Some(Event::Ping(&self.control)),
            10 => Some(Event::Pong),
            _ => unreachable!(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc_upgrade_requires_status_tokens_signature_and_no_extensions() {
        let nonce = *b"the sample nonce";
        let head=b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
        let mut h = Upgrade::new(&nonce);
        for (i, b) in head.iter().enumerate() {
            assert_eq!(h.push(*b), Ok(i + 1 == head.len()));
        }
        let mut h = Upgrade::new(&[1; 16]);
        assert!(head.iter().any(|b| h.push(*b).is_err()));
        let mut h = Upgrade::new(&nonce);
        assert!(
            b"HTTP/1.1 302 Redirect\r\nLocation: https://other.invalid\r\n\r\n"
                .iter()
                .any(|b| h.push(*b).is_err())
        );
    }
    #[test]
    fn fragmented_utf8_and_interleaved_ping_survive_single_byte_reads() {
        let mut d = Decoder::default();
        let mut events = Vec::new();
        for b in [0x01, 2, 0xe4, 0xbd, 0x89, 1, 7, 0x80, 1, 0xa0, 0x81, 0] {
            if let Some(e) = d.push(b).unwrap() {
                events.push(format!("{e:?}"));
            }
        }
        assert_eq!(events, ["Ping([7])", "Text(\"你\")", "Text(\"\")"]);
    }
    #[test]
    fn masking_lengths_control_frames_and_fragment_order_are_checked() {
        for frame in [
            &[0x81, 0x80][..],
            &[0x80, 0][..],
            &[0x09, 0][..],
            &[0x82, 0][..],
            &[0x81, 126, 0, 1][..],
            &[0x81, 127, 0, 0, 0, 0, 0, 1, 0, 0][..],
            &[0x88, 1, 0][..],
        ] {
            let mut d = Decoder::default();
            assert!(frame.iter().any(|b| d.push(*b).is_err()), "{frame:?}");
        }
        let mut out = Vec::new();
        encode(1, b"Hello", [0x37, 0xfa, 0x21, 0x3d], &mut out).unwrap();
        assert_eq!(
            out,
            [0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58]
        );
    }
}
