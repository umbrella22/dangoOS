//! File windows over JSON already validated by the incremental Responses parser.
//! Spans preserve provider identifiers and reasoning without allocating strings.
use fs::{Vfs, Whence};
pub(crate) const IO: &str = "AI 上下文存储失败，请检查 NAND 空间";
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    pub start: u32,
    pub end: u32,
}
pub(crate) struct Cursor<'a> {
    pub v: &'a mut Vfs,
    fd: i32,
    pub at: u32,
    bytes: [u8; 1024],
    start: u32,
    end: u32,
}
impl<'a> Cursor<'a> {
    pub fn new(v: &'a mut Vfs, fd: i32, at: u32) -> Self {
        Self {
            v,
            fd,
            at,
            bytes: [0; 1024],
            start: at,
            end: at,
        }
    }
    pub fn peek(&mut self) -> Result<Option<u8>, &'static str> {
        if self.at < self.start || self.at >= self.end {
            self.v
                .seek(self.fd, i64::from(self.at), Whence::Set)
                .map_err(|_| IO)?;
            let n = self.v.read(self.fd, &mut self.bytes).map_err(|_| IO)?;
            self.start = self.at;
            self.end = self.at.checked_add(n as u32).ok_or(IO)?;
            if n == 0 {
                return Ok(None);
            }
        }
        Ok(Some(self.bytes[(self.at - self.start) as usize]))
    }
    pub fn byte(&mut self) -> Result<u8, &'static str> {
        let b = self.peek()?.ok_or("AI 上下文 JSON 提前结束")?;
        self.at += 1;
        Ok(b)
    }
    pub fn whitespace(&mut self) -> Result<(), &'static str> {
        while self.peek()?.is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
        Ok(())
    }
    pub fn expect(&mut self, byte: u8) -> Result<(), &'static str> {
        self.whitespace()?;
        if self.byte()? != byte {
            return Err("AI 上下文 JSON 结构无效");
        }
        Ok(())
    }
    pub fn string(
        &mut self,
        mut emit: impl FnMut(&[u8]) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        self.expect(b'"')?;
        loop {
            let b = self.byte()?;
            match b {
                b'"' => return Ok(()),
                b'\\' => {
                    let b = self.byte()?;
                    match b {
                        b'"' | b'\\' | b'/' => emit(&[b])?,
                        b'b' => emit(&[8])?,
                        b'f' => emit(&[12])?,
                        b'n' => emit(b"\n")?,
                        b'r' => emit(b"\r")?,
                        b't' => emit(b"\t")?,
                        b'u' => {
                            let high = self.hex()?;
                            let code = if (0xd800..=0xdbff).contains(&high) {
                                if self.byte()? != b'\\' || self.byte()? != b'u' {
                                    return Err("AI JSON Unicode 无效");
                                }
                                let low = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err("AI JSON Unicode 无效");
                                }
                                0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
                            } else {
                                high
                            };
                            let c = char::from_u32(code).ok_or("AI JSON Unicode 无效")?;
                            emit(c.encode_utf8(&mut [0; 4]).as_bytes())?;
                        }
                        _ => return Err("AI JSON 转义无效"),
                    }
                }
                0..=31 => return Err("AI JSON 字符串无效"),
                _ => emit(&[b])?,
            }
        }
    }
    fn hex(&mut self) -> Result<u32, &'static str> {
        let mut value = 0;
        for _ in 0..4 {
            value = value * 16
                + match self.byte()? {
                    b @ b'0'..=b'9' => u32::from(b - b'0'),
                    b @ b'a'..=b'f' => u32::from(b - b'a' + 10),
                    b @ b'A'..=b'F' => u32::from(b - b'A' + 10),
                    _ => return Err("AI JSON Unicode 无效"),
                };
        }
        Ok(value)
    }
    pub fn key(&mut self) -> Result<Key, &'static str> {
        // Keys are matched against this module's finite field vocabulary.
        // Longer unknown keys are skipped, not truncated into a known key.
        let mut bytes = [0; 16];
        let mut n = 0usize;
        self.string(|s| {
            for b in s {
                if n < bytes.len() {
                    bytes[n] = *b;
                }
                n += 1;
            }
            Ok(())
        })?;
        self.expect(b':')?;
        Ok(match bytes.get(..n) {
            Some(b"type") => Key::Type,
            Some(b"name") => Key::Name,
            Some(b"call_id") => Key::Call,
            Some(b"arguments") => Key::Arguments,
            _ => Key::Other,
        })
    }
    pub fn span(&mut self) -> Result<Span, &'static str> {
        self.whitespace()?;
        let start = self.at;
        let mut depth = 0usize;
        let mut string = false;
        let mut escaped = false;
        loop {
            let Some(b) = self.peek()? else {
                break;
            };
            if !string && depth == 0 && (b.is_ascii_whitespace() || matches!(b, b',' | b']' | b'}'))
            {
                break;
            }
            self.at += 1;
            if string {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    string = false;
                }
            } else {
                match b {
                    b'"' => string = true,
                    b'[' | b'{' => depth += 1,
                    b']' | b'}' => depth = depth.checked_sub(1).ok_or("AI 上下文括号无效")?,
                    _ => (),
                }
            }
        }
        if start == self.at || string || depth != 0 {
            return Err("AI 上下文值无效");
        }
        Ok(Span {
            start,
            end: self.at,
        })
    }
    pub fn equals(&mut self, span: Span, text: &str) -> Result<bool, &'static str> {
        self.at = span.start;
        let mut n = 0;
        let mut same = true;
        self.string(|s| {
            for b in s {
                same &= text.as_bytes().get(n) == Some(b);
                n += 1;
            }
            Ok(())
        })?;
        Ok(same && n == text.len() && self.at == span.end)
    }
}
pub(crate) enum Key {
    Type,
    Name,
    Call,
    Arguments,
    Other,
}
