//! SSE lines/events are unrelated to HTTP chunks and may split UTF-8 bytes.
#[cfg(not(target_os = "none"))]
use alloc::vec::Vec;
pub use voice_session::sse::{Framer, Token};
#[cfg(not(target_os = "none"))]
#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    data: Vec<u8>,
    cr: bool,
    clear: bool,
    has_data: bool,
}
#[cfg(not(target_os = "none"))]
impl Decoder {
    pub fn push(&mut self, byte: u8) -> Result<Option<&[u8]>, &'static str> {
        if self.clear {
            self.data.clear();
            self.has_data = false;
            self.clear = false;
        }
        if self.cr {
            self.cr = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        if !matches!(byte, b'\r' | b'\n') {
            self.line.push(byte);
            return Ok(None);
        }
        self.cr = byte == b'\r';
        if self.line.is_empty() {
            if self.has_data {
                self.clear = true;
                return Ok(Some(&self.data));
            }
            return Ok(None);
        }
        if self.line == b"data" || self.line.starts_with(b"data:") {
            let value = self.line.get(5..).unwrap_or(&[]);
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if self.has_data {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(value);
            self.has_data = true;
        }
        self.line.clear();
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comments_crlf_cr_and_multiple_data_lines_are_network_independent() {
        let mut d = Decoder::default();
        let mut out = Vec::new();
        for b in b":heartbeat\r\nevent: message\r\ndata: \xe4\xbd\xa0\r\ndata: hi\r\n\r\ndata: [DONE]\r\r" {
            if let Some(e)=d.push(*b).unwrap(){out.push(e.to_vec());}
        }
        assert_eq!(out, ["你\nhi".as_bytes(), b"[DONE]"]);
    }
    #[test]
    fn long_comments_and_events_have_no_content_cap_in_the_host_test_adapter() {
        let mut d = Decoder::default();
        for b in b":comment"
            .iter()
            .copied()
            .chain(core::iter::repeat_n(b'x', 400_000))
            .chain(b"\n\ndata:".iter().copied())
            .chain(core::iter::repeat_n(b'y', 300_000))
            .chain(b"\n\n".iter().copied())
        {
            if let Some(data) = d.push(b).unwrap() {
                assert_eq!(data.len(), 300_000);
                return;
            }
        }
        panic!("missing event");
    }
}
