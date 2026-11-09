//! Incremental JSON projection. Large strings are decoded a scalar at a time;
//! only protocol metadata and text spans survive an event.
use alloc::{string::String, vec::Vec};
const INVALID: &str = "AI 流 JSON 无效";
#[derive(Clone)]
pub struct TextSpan {
    pub bytes: usize,
    pub visible: bool,
    pub verified: bool,
}
impl Default for TextSpan {
    fn default() -> Self {
        Self {
            bytes: 0,
            visible: false,
            verified: true,
        }
    }
}
impl TextSpan {
    pub fn push(&mut self, bytes: &[u8], verified: bool) {
        self.bytes += bytes.len();
        self.verified &= verified;
        self.visible |=
            core::str::from_utf8(bytes).is_ok_and(|s| s.chars().any(|c| !c.is_whitespace()));
    }
    pub fn same(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.verified && other.verified
    }
}
pub enum TextSource {
    Append,
    Compare(usize),
    OutputStart,
    Output,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Key {
    #[default]
    Other,
    Type,
    Id,
    Role,
    Status,
    Response,
    Item,
    Output,
    Content,
    Incomplete,
    Reason,
    Error,
    Delta,
    Text,
    Refusal,
    Sequence,
    ItemId,
    OutputIndex,
    ContentIndex,
    Percent,
}
impl Key {
    #[inline(never)]
    fn parse(s: &str) -> Self {
        match s {
            "type" => Self::Type,
            "id" => Self::Id,
            "role" => Self::Role,
            "status" => Self::Status,
            "response" => Self::Response,
            "item" => Self::Item,
            "output" => Self::Output,
            "content" => Self::Content,
            "incomplete_details" => Self::Incomplete,
            "reason" => Self::Reason,
            "error" => Self::Error,
            "delta" => Self::Delta,
            "text" => Self::Text,
            "refusal" => Self::Refusal,
            "sequence_number" => Self::Sequence,
            "item_id" => Self::ItemId,
            "output_index" => Self::OutputIndex,
            "content_index" => Self::ContentIndex,
            "percent" => Self::Percent,
            _ => Self::Other,
        }
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Role {
    #[default]
    Other,
    Root,
    Response,
    Item,
    Output,
    OutputItem,
    Content,
    Part,
    Incomplete,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Other,
    Message,
    Reasoning,
    Text,
    Refusal,
    ReasoningText,
    Function,
}
#[derive(Default)]
struct Frame {
    role: Role,
    // Object: 0 key/end, 1 colon, 2 value, 3 comma/end, 4 key.
    // Array: 5 value/end, 6 comma/end, 7 value.
    state: u8,
    key: Key,
    kind: Kind,
    assistant: bool,
    completed: bool,
    content_seen: bool,
    reasoning_content: bool,
    text: Option<TextSpan>,
}
#[derive(Default)]
pub struct Event {
    pub kind: String,
    pub sequence: Option<u64>,
    pub response: String,
    pub item: String,
    pub output: Option<u64>,
    pub content: Option<u64>,
    pub completed: bool,
    pub error: bool,
    pub incomplete: bool,
    pub reason: String,
    pub unsupported: bool,
    pub bad_item: bool,
    pub text: Option<TextSpan>,
    pub snapshot: Vec<TextSpan>,
    pub snapshot_seen: bool,
    pub item_seen: bool,
    pub function_calls: usize,
    pub percent: Option<u64>,
    pub argument_fields: usize,
    pub invalid_arguments: bool,
}
#[derive(Default)]
pub struct Json {
    frames: Vec<Frame>,
    lex: Lex,
    root: bool,
    finished: bool,
    capture: String,
    key_overflow: bool,
    span: Option<TextSpan>,
    seed: TextSpan,
    text_seed: TextSpan,
    snapshot_span: TextSpan,
    pub event: Event,
    archive_depth: Option<usize>,
    arguments: bool,
}
/// Exact decimal projection for the device's integer percentage domain. Three
/// significant digits suffice for 0..=100; trailing zeros and exponent spelling
/// never require storing the numerical token itself.
#[derive(Default)]
struct Percentage {
    negative: bool,
    prefix: u16,
    digits: u64,
    last_nonzero: u64,
    fraction: u64,
    exponent: u64,
    exponent_overflow: bool,
    exponent_negative: bool,
}
impl Percentage {
    fn push(&mut self, state: u8, b: u8) {
        if state == 0 && b == b'-' {
            self.negative = true;
        }
        if state == 6 && b == b'-' {
            self.exponent_negative = true;
        }
        if !b.is_ascii_digit() {
            return;
        }
        let digit = u16::from(b - b'0');
        if state >= 6 {
            match self
                .exponent
                .checked_mul(10)
                .and_then(|n| n.checked_add(u64::from(digit)))
            {
                Some(n) => self.exponent = n,
                None => self.exponent_overflow = true,
            }
        } else {
            if state == 4 || state == 5 {
                self.fraction += 1;
            }
            if digit != 0 || self.digits != 0 {
                self.digits += 1;
                if self.digits <= 3 {
                    self.prefix = self.prefix * 10 + digit;
                }
                if digit != 0 {
                    self.last_nonzero = self.digits;
                }
            }
        }
    }
    fn value(self) -> Option<u64> {
        if self.digits == 0 {
            return Some(0);
        }
        if self.negative || self.last_nonzero > 3 || self.exponent_overflow {
            return None;
        }
        let exponent = i128::from(self.exponent) * if self.exponent_negative { -1 } else { 1 };
        let scale =
            i128::from(self.digits - self.last_nonzero) - i128::from(self.fraction) + exponent;
        if !(0..=2).contains(&scale) {
            return None;
        }
        let unused = self.digits.min(3) - self.last_nonzero;
        let coefficient = u64::from(self.prefix) / 10u64.pow(unused as u32);
        coefficient
            .checked_mul(10u64.pow(scale as u32))
            .filter(|n| *n <= 100)
    }
}
#[derive(Default)]
enum Lex {
    #[default]
    None,
    String {
        key: bool,
        escape: Escape,
        utf8: [u8; 4],
        used: u8,
        needed: u8,
    },
    Number {
        state: u8,
        value: Option<u64>,
        unsigned: bool,
        percent: Percentage,
    },
    Literal {
        word: &'static [u8],
        at: usize,
    },
}
#[derive(Default)]
enum Escape {
    #[default]
    None,
    Slash,
    Hex(u16, u8),
    PairSlash(u16),
    PairU(u16),
    PairHex(u16, u16, u8),
}
impl Json {
    pub fn arguments() -> Self {
        Self {
            arguments: true,
            ..Self::default()
        }
    }
    pub fn snapshot_base(mut self, base: TextSpan) -> Self {
        self.snapshot_span = base;
        self
    }
    pub fn new(seed: TextSpan, text_seed: TextSpan) -> Self {
        Self {
            seed,
            text_seed,
            ..Self::default()
        }
    }
    #[inline(never)]
    fn context(&self) -> (Role, Key) {
        self.frames
            .last()
            .map_or((Role::Other, Key::Other), |f| (f.role, f.key))
    }
    #[inline(never)]
    fn value(&mut self, object: bool) -> Result<(), &'static str> {
        let (role, key) = self.context();
        if matches!(role, Role::Output | Role::Content) && !object {
            self.event.unsupported = true;
        }
        if role == Role::Response && key == Key::Error {
            self.event.error = true;
        }
        if role == Role::Response && key == Key::Incomplete {
            self.event.incomplete = true;
        }
        if let Some(f) = self.frames.last_mut() {
            f.state = match f.state {
                2 => 3,
                5 | 7 => 6,
                _ => return Err(INVALID),
            };
        } else if self.root {
            return Err(INVALID);
        } else {
            self.root = true;
        }
        Ok(())
    }
    #[inline(never)]
    fn container(&mut self, array: bool) -> Result<(), &'static str> {
        let (parent, key) = self.context();
        if self.arguments && !self.root && array {
            self.event.invalid_arguments = true;
        }
        let role = if !self.root {
            Role::Root
        } else {
            match (parent, key, array) {
                (Role::Root, Key::Response, false) => Role::Response,
                (Role::Root, Key::Item, false) => Role::Item,
                (Role::Response, Key::Output, true) => {
                    self.event.snapshot_seen = true;
                    Role::Output
                }
                (Role::Output, _, false) => Role::OutputItem,
                (Role::Item | Role::OutputItem, Key::Content, true) => Role::Content,
                (Role::Content, _, false) => Role::Part,
                (Role::Response, Key::Incomplete, false) => Role::Incomplete,
                _ => Role::Other,
            }
        };
        if parent == Role::Response && key == Key::Error {
            self.event.error = true;
        }
        if parent == Role::Response && key == Key::Incomplete {
            self.event.incomplete = true;
        }
        self.value(!array)?;
        self.frames.try_reserve(1).map_err(|_| "AI 解析内存不足")?;
        self.frames.push(Frame {
            role,
            state: if array { 5 } else { 0 },
            ..Frame::default()
        });
        Ok(())
    }
    #[inline(never)]
    fn close(&mut self, array: bool) -> Result<(), &'static str> {
        let f = self.frames.pop().ok_or(INVALID)?;
        if !if array {
            matches!(f.state, 5 | 6)
        } else {
            matches!(f.state, 0 | 3)
        } {
            return Err(INVALID);
        }
        match f.role {
            Role::Content => {
                if let Some(parent) = self.frames.last_mut() {
                    parent.content_seen = true;
                    parent.reasoning_content = f.reasoning_content;
                }
            }
            Role::Part => match f.kind {
                Kind::Text | Kind::Refusal => {
                    self.event
                        .snapshot
                        .try_reserve(1)
                        .map_err(|_| "AI 解析内存不足")?;
                    self.snapshot_span = f.text.ok_or(INVALID)?;
                    self.event.snapshot.push(self.snapshot_span.clone());
                }
                Kind::ReasoningText => {
                    if let Some(parent) = self.frames.last_mut() {
                        parent.reasoning_content = true;
                    }
                }
                _ => self.event.unsupported = true,
            },
            Role::Item | Role::OutputItem => {
                self.event.item_seen = true;
                match f.kind {
                    Kind::Reasoning => (),
                    Kind::Function => {
                        if f.role == Role::OutputItem {
                            self.event.function_calls += 1;
                            self.event.bad_item |= !f.completed;
                        }
                    }
                    Kind::Message => {
                        self.event.unsupported |= !f.assistant;
                        self.event.bad_item |=
                            f.role == Role::OutputItem && (!f.completed || !f.content_seen);
                        self.event.unsupported |= f.reasoning_content;
                    }
                    _ => self.event.unsupported = true,
                }
            }
            _ => (),
        }
        if self.frames.is_empty() {
            self.finished = true;
        }
        Ok(())
    }
    #[inline(never)]
    fn scalar(
        &mut self,
        c: char,
        is_key: bool,
        write: &mut impl FnMut(TextSource, &[u8]) -> Result<bool, &'static str>,
    ) -> Result<(), &'static str> {
        if is_key {
            if self.capture.len() < 32 {
                self.capture.push(c)
            } else {
                self.key_overflow = true;
            }
            return Ok(());
        }
        let (role, key) = self.context();
        let mut raw = [0; 4];
        let bytes = c.encode_utf8(&mut raw).as_bytes();
        if let Some(span) = self.span.as_mut() {
            if c == '\0' {
                return Err("语音服务返回了无效文字");
            }
            let verified = if role == Role::Root && key == Key::Delta {
                if self.event.kind.is_empty()
                    || matches!(
                        self.event.kind.as_str(),
                        "response.output_text.delta" | "response.refusal.delta"
                    )
                {
                    write(TextSource::Append, bytes)?
                } else {
                    true
                }
            } else if span.verified {
                write(TextSource::Compare(span.bytes), bytes)?
            } else {
                false
            };
            span.push(bytes, verified);
        }
        let metadata = matches!(
            (role, key),
            (Role::Root, Key::Type | Key::ItemId)
                | (Role::Response, Key::Id | Key::Status)
                | (
                    Role::Item | Role::OutputItem,
                    Key::Type | Key::Role | Key::Status
                )
                | (Role::Part, Key::Type)
                | (Role::Incomplete, Key::Reason)
        );
        if metadata {
            if self.capture.len() + bytes.len() > 128 {
                return Err("AI 流元数据过长");
            }
            self.capture.push(c);
        }
        Ok(())
    }
    #[inline(never)]
    fn string_end(&mut self, is_key: bool) -> Result<(), &'static str> {
        if is_key {
            let key = if self.key_overflow {
                Key::Other
            } else {
                Key::parse(&self.capture)
            };
            let f = self.frames.last_mut().ok_or(INVALID)?;
            f.key = key;
            f.state = 1;
            if self.arguments && f.role == Role::Root {
                self.event.argument_fields += 1;
                self.event.invalid_arguments |=
                    key != Key::Percent || self.event.argument_fields > 1;
            }
        } else {
            let (role, key) = self.context();
            let s = self.capture.as_str();
            match (role, key) {
                (Role::Root, Key::Type) => self.event.kind.push_str(s),
                (Role::Root, Key::ItemId) => self.event.item.push_str(s),
                (Role::Response, Key::Id) => self.event.response.push_str(s),
                (Role::Response, Key::Status) => self.event.completed = s == "completed",
                (Role::Incomplete, Key::Reason) => self.event.reason.push_str(s),
                (Role::Item | Role::OutputItem | Role::Part, Key::Type) => {
                    self.frames.last_mut().unwrap().kind = match s {
                        "message" => Kind::Message,
                        "reasoning" => Kind::Reasoning,
                        "output_text" => Kind::Text,
                        "refusal" => Kind::Refusal,
                        "reasoning_text" => Kind::ReasoningText,
                        "function_call" => Kind::Function,
                        _ => Kind::Other,
                    };
                }
                (Role::Item | Role::OutputItem, Key::Role) => {
                    self.frames.last_mut().unwrap().assistant = s == "assistant"
                }
                (Role::Item | Role::OutputItem, Key::Status) => {
                    self.frames.last_mut().unwrap().completed = s == "completed"
                }
                _ => (),
            }
            if let Some(hash) = self.span.take() {
                if role == Role::Root {
                    self.event.text = Some(hash)
                } else {
                    self.frames.last_mut().unwrap().text = Some(hash)
                }
            }
        }
        self.capture.clear();
        self.key_overflow = false;
        Ok(())
    }
    #[inline(never)]
    pub fn push(
        &mut self,
        b: u8,
        write: &mut impl FnMut(TextSource, &[u8]) -> Result<bool, &'static str>,
    ) -> Result<(), &'static str> {
        if self.archive_depth.is_none()
            && matches!(self.lex, Lex::None)
            && self.context() == (Role::Response, Key::Output)
            && b == b'['
        {
            write(TextSource::OutputStart, &[])?;
            self.archive_depth = Some(self.frames.len() + 1);
        }
        if self.archive_depth.is_some() {
            write(TextSource::Output, &[b])?;
        }
        self.push_inner(b, write)?;
        if self
            .archive_depth
            .is_some_and(|depth| self.frames.len() < depth)
        {
            self.archive_depth = None;
        }
        Ok(())
    }
    fn push_inner(
        &mut self,
        b: u8,
        write: &mut impl FnMut(TextSource, &[u8]) -> Result<bool, &'static str>,
    ) -> Result<(), &'static str> {
        let lex = core::mem::take(&mut self.lex);
        match lex {
            Lex::String {
                key,
                mut escape,
                mut utf8,
                mut used,
                mut needed,
            } => {
                let mut scalar = None;
                escape = match escape {
                    Escape::None => {
                        if needed != 0 {
                            if !(0x80..=0xbf).contains(&b) {
                                return Err(INVALID);
                            }
                            utf8[used as usize] = b;
                            used += 1;
                            if used == needed {
                                scalar = core::str::from_utf8(&utf8[..used as usize])
                                    .map_err(|_| INVALID)?
                                    .chars()
                                    .next();
                                needed = 0;
                                used = 0;
                            }
                        } else {
                            match b {
                                b'"' => {
                                    self.string_end(key)?;
                                    return Ok(());
                                }
                                b'\\' => {
                                    self.lex = Lex::String {
                                        key,
                                        escape: Escape::Slash,
                                        utf8,
                                        used,
                                        needed,
                                    };
                                    return Ok(());
                                }
                                0..=0x1f => return Err(INVALID),
                                0x20..=0x7f => scalar = Some(b as char),
                                0xc2..=0xdf => {
                                    utf8[0] = b;
                                    used = 1;
                                    needed = 2;
                                }
                                0xe0..=0xef => {
                                    utf8[0] = b;
                                    used = 1;
                                    needed = 3;
                                }
                                0xf0..=0xf4 => {
                                    utf8[0] = b;
                                    used = 1;
                                    needed = 4;
                                }
                                _ => return Err(INVALID),
                            }
                        }
                        Escape::None
                    }
                    Escape::Slash => match b {
                        b'"' | b'\\' | b'/' => {
                            scalar = Some(b as char);
                            Escape::None
                        }
                        b'b' => {
                            scalar = Some('\u{8}');
                            Escape::None
                        }
                        b'f' => {
                            scalar = Some('\u{c}');
                            Escape::None
                        }
                        b'n' => {
                            scalar = Some('\n');
                            Escape::None
                        }
                        b'r' => {
                            scalar = Some('\r');
                            Escape::None
                        }
                        b't' => {
                            scalar = Some('\t');
                            Escape::None
                        }
                        b'u' => Escape::Hex(0, 0),
                        _ => return Err(INVALID),
                    },
                    Escape::Hex(v, n) => {
                        let v = (v << 4) | hex(b)?;
                        let n = n + 1;
                        if n < 4 {
                            Escape::Hex(v, n)
                        } else if (0xd800..=0xdbff).contains(&v) {
                            Escape::PairSlash(v)
                        } else {
                            scalar = Some(char::from_u32(v as u32).ok_or(INVALID)?);
                            Escape::None
                        }
                    }
                    Escape::PairSlash(high) => {
                        if b == b'\\' {
                            Escape::PairU(high)
                        } else {
                            return Err(INVALID);
                        }
                    }
                    Escape::PairU(high) => {
                        if b == b'u' {
                            Escape::PairHex(high, 0, 0)
                        } else {
                            return Err(INVALID);
                        }
                    }
                    Escape::PairHex(high, v, n) => {
                        let v = (v << 4) | hex(b)?;
                        let n = n + 1;
                        if n < 4 {
                            Escape::PairHex(high, v, n)
                        } else {
                            if !(0xdc00..=0xdfff).contains(&v) {
                                return Err(INVALID);
                            }
                            scalar = Some(
                                char::from_u32(
                                    0x10000
                                        + ((u32::from(high) - 0xd800) << 10)
                                        + (u32::from(v) - 0xdc00),
                                )
                                .ok_or(INVALID)?,
                            );
                            Escape::None
                        }
                    }
                };
                if let Some(c) = scalar {
                    self.scalar(c, key, write)?;
                }
                self.lex = Lex::String {
                    key,
                    escape,
                    utf8,
                    used,
                    needed,
                };
                Ok(())
            }
            Lex::Number {
                mut state,
                mut value,
                mut unsigned,
                mut percent,
            } => {
                if matches!(b, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    percent.push(state, b);
                    state = match (state, b) {
                        (0, b'-') => {
                            unsigned = false;
                            1
                        }
                        (0 | 1, b'0') => 2,
                        (0 | 1, b'1'..=b'9') => 3,
                        (3, b'0'..=b'9') => 3,
                        (2 | 3, b'.') => {
                            unsigned = false;
                            4
                        }
                        (4 | 5, b'0'..=b'9') => 5,
                        (2 | 3 | 5, b'e' | b'E') => {
                            unsigned = false;
                            6
                        }
                        (6, b'+' | b'-') => 7,
                        (6 | 7 | 8, b'0'..=b'9') => 8,
                        _ => return Err(INVALID),
                    };
                    if b.is_ascii_digit() {
                        value = value
                            .and_then(|n| n.checked_mul(10))
                            .and_then(|n| n.checked_add(u64::from(b - b'0')));
                    }
                    self.lex = Lex::Number {
                        state,
                        value,
                        unsigned,
                        percent,
                    };
                    return Ok(());
                }
                if !matches!(state, 2 | 3 | 5 | 8) {
                    return Err(INVALID);
                }
                let (role, key) = self.context();
                if role == Role::Root {
                    let value = if unsigned { value } else { None };
                    match key {
                        Key::Sequence => self.event.sequence = value,
                        Key::OutputIndex => self.event.output = value,
                        Key::ContentIndex => self.event.content = value,
                        Key::Percent if self.arguments => self.event.percent = percent.value(),
                        _ => (),
                    }
                }
                self.push_inner(b, write)
            }
            Lex::Literal { word, at } => {
                if word.get(at) != Some(&b) {
                    return Err(INVALID);
                }
                if at + 1 < word.len() {
                    self.lex = Lex::Literal { word, at: at + 1 };
                }
                Ok(())
            }
            Lex::None => {
                if b.is_ascii_whitespace() {
                    return Ok(());
                }
                if self.finished {
                    return Err(INVALID);
                }
                match b {
                    b'{' => self.container(false),
                    b'[' => self.container(true),
                    b'}' => self.close(false),
                    b']' => self.close(true),
                    b':' => {
                        let f = self.frames.last_mut().ok_or(INVALID)?;
                        if f.state != 1 {
                            return Err(INVALID);
                        }
                        f.state = 2;
                        Ok(())
                    }
                    b',' => {
                        let f = self.frames.last_mut().ok_or(INVALID)?;
                        f.state = match f.state {
                            3 => 4,
                            6 => 7,
                            _ => return Err(INVALID),
                        };
                        Ok(())
                    }
                    b'"' => {
                        let key = self.frames.last().is_some_and(|f| matches!(f.state, 0 | 4));
                        if !key {
                            self.value(false)?;
                            let (role, k) = self.context();
                            if matches!(
                                (role, k),
                                (Role::Root, Key::Delta | Key::Text | Key::Refusal)
                                    | (Role::Part, Key::Text | Key::Refusal)
                            ) {
                                self.span = Some(if role == Role::Part {
                                    self.snapshot_span.clone()
                                } else if k == Key::Delta {
                                    self.seed.clone()
                                } else {
                                    self.text_seed.clone()
                                });
                            }
                        }
                        self.lex = Lex::String {
                            key,
                            escape: Escape::None,
                            utf8: [0; 4],
                            used: 0,
                            needed: 0,
                        };
                        Ok(())
                    }
                    b't' | b'f' | b'n' => {
                        let (role, key) = self.context();
                        self.value(false)?;
                        if role == Role::Response && key == Key::Error {
                            self.event.error = b != b'n';
                        }
                        if role == Role::Response && key == Key::Incomplete {
                            self.event.incomplete = b != b'n';
                        }
                        self.lex = Lex::Literal {
                            word: match b {
                                b't' => b"true",
                                b'f' => b"false",
                                _ => b"null",
                            },
                            at: 1,
                        };
                        Ok(())
                    }
                    b'-' | b'0'..=b'9' => {
                        self.value(false)?;
                        self.lex = Lex::Number {
                            state: 0,
                            value: Some(0),
                            unsigned: true,
                            percent: Percentage::default(),
                        };
                        self.push_inner(b, write)
                    }
                    _ => Err(INVALID),
                }
            }
        }
    }
    #[inline(never)]
    #[cfg(not(target_os = "none"))]
    pub fn buffered_bytes(&self) -> usize {
        self.frames.capacity() * core::mem::size_of::<Frame>()
            + self.capture.capacity()
            + self.event.kind.capacity()
            + self.event.response.capacity()
            + self.event.item.capacity()
            + self.event.reason.capacity()
            + self.event.snapshot.capacity() * core::mem::size_of::<TextSpan>()
    }
    pub fn finish(self) -> Result<Event, &'static str> {
        if !self.finished || !matches!(self.lex, Lex::None) {
            return Err(INVALID);
        }
        Ok(self.event)
    }
}
fn hex(b: u8) -> Result<u16, &'static str> {
    match b {
        b'0'..=b'9' => Ok(u16::from(b - b'0')),
        b'a'..=b'f' => Ok(u16::from(b - b'a' + 10)),
        b'A'..=b'F' => Ok(u16::from(b - b'A' + 10)),
        _ => Err(INVALID),
    }
}
