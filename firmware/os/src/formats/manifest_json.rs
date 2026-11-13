//! A small JSON reader for `launcher.json` manifests (RFC 8259). Objects
//! keep their members in order; every value remembers its source span so
//! `args` can be handed to Lua as the original text.

use alloc::{string::String, vec::Vec};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<Spanned>),
    Object(Vec<(String, Spanned)>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spanned {
    pub value: Value,
    /// Byte range of the value in the input.
    pub start: usize,
    pub end: usize,
}

impl Spanned {
    /// Member `key` of an object; the first one when repeated, as cJSON
    /// (which the official registry uses) finds it.
    pub fn get(&self, key: &str) -> Option<&Spanned> {
        match &self.value {
            Value::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Nesting limit, so a hostile manifest cannot exhaust the stack.
const MAX_DEPTH: usize = 32;

pub fn parse(text: &str) -> Result<Spanned, String> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        alloc::format!("{what} at byte {}", self.i)
    }

    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.s.get(self.i) {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &[u8]) -> bool {
        if self.s[self.i..].starts_with(lit) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Spanned, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        let start = self.i;
        let value = match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if !self.eat(b"}") {
                    loop {
                        self.ws();
                        if self.s.get(self.i) != Some(&b'"') {
                            return Err(self.err("expected a member name"));
                        }
                        let key = self.string()?;
                        self.ws();
                        if !self.eat(b":") {
                            return Err(self.err("expected ':'"));
                        }
                        self.ws();
                        let v = self.value(depth + 1)?;
                        members.push((key, v));
                        self.ws();
                        if self.eat(b",") {
                            continue;
                        }
                        if self.eat(b"}") {
                            break;
                        }
                        return Err(self.err("expected ',' or '}'"));
                    }
                }
                Value::Object(members)
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if !self.eat(b"]") {
                    loop {
                        self.ws();
                        items.push(self.value(depth + 1)?);
                        self.ws();
                        if self.eat(b",") {
                            continue;
                        }
                        if self.eat(b"]") {
                            break;
                        }
                        return Err(self.err("expected ',' or ']'"));
                    }
                }
                Value::Array(items)
            }
            Some(b'"') => Value::Str(self.string()?),
            Some(b't') if self.eat(b"true") => Value::Bool(true),
            Some(b'f') if self.eat(b"false") => Value::Bool(false),
            Some(b'n') if self.eat(b"null") => Value::Null,
            Some(b'-' | b'0'..=b'9') => Value::Number(self.number()?),
            _ => return Err(self.err("expected a value")),
        };
        Ok(Spanned {
            value,
            start,
            end: self.i,
        })
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.i;
        self.eat(b"-");
        let digits = |p: &mut Self| {
            let s = p.i;
            while let Some(b'0'..=b'9') = p.s.get(p.i) {
                p.i += 1;
            }
            p.i - s
        };
        if self.eat(b"0") {
        } else if digits(self) == 0 {
            return Err(self.err("bad number"));
        }
        if self.eat(b".") && digits(self) == 0 {
            return Err(self.err("bad fraction"));
        }
        if let Some(b'e' | b'E') = self.s.get(self.i) {
            self.i += 1;
            if let Some(b'+' | b'-') = self.s.get(self.i) {
                self.i += 1;
            }
            if digits(self) == 0 {
                return Err(self.err("bad exponent"));
            }
        }
        let text = core::str::from_utf8(&self.s[start..self.i]).unwrap();
        text.parse::<f64>().map_err(|_| self.err("bad number"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self
            .s
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("short \\u escape"))?;
        let h = core::str::from_utf8(h).map_err(|_| self.err("bad \\u escape"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let run = self.i;
            while let Some(&b) = self.s.get(self.i) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.i += 1;
            }
            // The input is a &str, and the run stops at ASCII bytes only.
            out.push_str(core::str::from_utf8(&self.s[run..self.i]).unwrap());
            match self.s.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = match self.s.get(self.i) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.i += 1;
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if !self.eat(b"\\u") {
                                    return Err(self.err("lone surrogate"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("bad surrogate pair"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            out.push(
                                char::from_u32(code).ok_or_else(|| self.err("bad code point"))?,
                            );
                            continue;
                        }
                        _ => return Err(self.err("bad escape")),
                    };
                    self.i += 1;
                    out.push(c);
                }
                _ => return Err(self.err("unterminated string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifests_with_spans_and_escapes() {
        let text = r#" {"id":"a\u00e9\ud83e\udd8a","n":-1.5e2,"ok":true,"z":null,
            "args":{"k":[1,2,{"x":"y"}]}} "#;
        let v = parse(text).unwrap();
        assert_eq!(
            v.get("id").unwrap().value,
            Value::Str("a\u{e9}\u{1f98a}".into())
        );
        assert_eq!(v.get("n").unwrap().value, Value::Number(-150.0));
        assert_eq!(v.get("ok").unwrap().value, Value::Bool(true));
        assert_eq!(v.get("z").unwrap().value, Value::Null);
        let args = v.get("args").unwrap();
        assert_eq!(&text[args.start..args.end], r#"{"k":[1,2,{"x":"y"}]}"#);
        assert!(v.get("missing").is_none());
    }

    #[test]
    fn rejects_malformed_json() {
        for bad in [
            "",
            "{",
            "{\"a\"}",
            "{\"a\":}",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "\"\\x\"",
            "\"\\ud800\"",
            "\"a",
            "{} x",
            "tru",
            "\"tab\there\"",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let deep = "[".repeat(40) + &"]".repeat(40);
        assert!(parse(&deep).is_err());
    }
}
