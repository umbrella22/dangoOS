//! Reversible byte fields for console commands and persisted radio settings.
use alloc::{string::String, vec::Vec};

pub fn decode(text: &str, max: usize) -> Result<Vec<u8>, &'static str> {
    if text.len() % 2 != 0 || text.len() / 2 > max {
        return Err("invalid hex field length");
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let nibble = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                b'A'..=b'F' => Some(c - b'A' + 10),
                _ => None,
            };
            Ok(nibble(pair[0]).ok_or("invalid hex field")? * 16
                + nibble(pair[1]).ok_or("invalid hex field")?)
        })
        .collect()
}

pub fn encode(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
