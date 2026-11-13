//! Current-task context on NAND. Provider output is replayed byte-for-byte;
//! RAM contains file windows and one typed device action.
use crate::{
    cursor::{Cursor, Key, Span, IO},
    json_stream::{Json, TextSource},
};
use alloc::vec::Vec;
use fs::{OpenFlags, Vfs, Whence};
const CONTEXT: &str = "/data/voice/context.json";
const OUTPUT: &str = "/data/voice/output.json";
pub const TOOLS: &str = concat!(
    "[{\"type\":\"function\",\"name\":\"device_status\",\"description\":\"Read current battery, speaker volume and display brightness.\",\"strict\":true,\"parameters\":{\"type\":\"object\",\"properties\":{},\"required\":[],\"additionalProperties\":false}},",
    "{\"type\":\"function\",\"name\":\"set_volume\",\"description\":\"Set system speaker volume in percent. Hardware output protection still applies.\",\"strict\":true,\"parameters\":{\"type\":\"object\",\"properties\":{\"percent\":{\"type\":\"integer\",\"minimum\":0,\"maximum\":100}},\"required\":[\"percent\"],\"additionalProperties\":false}},",
    "{\"type\":\"function\",\"name\":\"set_brightness\",\"description\":\"Set display brightness from 10 to 100 percent.\",\"strict\":true,\"parameters\":{\"type\":\"object\",\"properties\":{\"percent\":{\"type\":\"integer\",\"minimum\":10,\"maximum\":100}},\"required\":[\"percent\"],\"additionalProperties\":false}}]"
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Status,
    Volume(u8),
    Brightness(u8),
}
#[derive(Debug)]
pub struct Call {
    pub action: Result<Action, &'static str>,
    id: Span,
}
pub struct Agent {
    context: Option<i32>,
    output: Option<i32>,
    defer_close: fn(i32),
    pending: Vec<u8>,
    cursor: u32,
    outstanding: bool,
    ready: bool,
}
impl Agent {
    pub fn new(v: &mut Vfs, defer_close: fn(i32), user_json: &[u8]) -> Result<Self, &'static str> {
        let flags = OpenFlags {
            read: true,
            ..OpenFlags::WRITE
        };
        let mut s = Self {
            context: None,
            output: None,
            defer_close,
            pending: Vec::with_capacity(1024),
            cursor: 1,
            outstanding: false,
            ready: false,
        };
        s.context = Some(v.open(CONTEXT, flags).map_err(|_| IO)?);
        s.output = Some(match v.open(OUTPUT, flags) {
            Ok(fd) => fd,
            Err(_) => {
                let _ = v.close(s.context.take().unwrap());
                let _ = v.unlink(CONTEXT);
                return Err(IO);
            }
        });
        let result = write_all(v, s.context.unwrap(), user_json);
        if result.is_err() {
            let _ = s.close(v);
            return Err(IO);
        }
        Ok(s)
    }
    pub(crate) fn archive(
        &mut self,
        v: &mut Vfs,
        source: TextSource,
        bytes: &[u8],
    ) -> Result<bool, &'static str> {
        let fd = self.output.ok_or(IO)?;
        match source {
            TextSource::OutputStart => {
                self.ready = false;
                self.pending.clear();
                v.ftruncate(fd, 0).map_err(|_| IO)?;
                v.seek(fd, 0, Whence::Set).map_err(|_| IO)?;
                self.cursor = 1;
            }
            TextSource::Output => {
                if self.pending.len() + bytes.len() > 1024 {
                    self.flush(v)?;
                }
                self.pending.extend_from_slice(bytes);
            }
            _ => unreachable!(),
        }
        Ok(true)
    }
    fn flush(&mut self, v: &mut Vfs) -> Result<(), &'static str> {
        write_all(v, self.output.ok_or(IO)?, &self.pending)?;
        self.pending.clear();
        Ok(())
    }
    pub fn context_bytes(&mut self, v: &mut Vfs) -> Result<u32, &'static str> {
        let fd = self.context.ok_or(IO)?;
        v.fsync(fd).map_err(|_| IO)?;
        Ok(v.fstat(fd).map_err(|_| IO)?.size)
    }
    pub fn read_context(
        &mut self,
        v: &mut Vfs,
        offset: u32,
        bytes: &mut [u8],
    ) -> Result<usize, &'static str> {
        let fd = self.context.ok_or(IO)?;
        v.seek(fd, i64::from(offset), Whence::Set).map_err(|_| IO)?;
        v.read(fd, bytes).map_err(|_| IO)
    }
    pub fn commit_round(&mut self, v: &mut Vfs) -> Result<(), &'static str> {
        self.flush(v)?;
        let output = self.output.ok_or(IO)?;
        let context = self.context.ok_or(IO)?;
        v.fsync(output).map_err(|_| IO)?;
        let size = v.fstat(output).map_err(|_| IO)?.size;
        let mut c = Cursor::new(v, output, 0);
        c.expect(b'[')?;
        c.at = size.checked_sub(1).ok_or(IO)?;
        c.expect(b']')?;
        c.v.seek(context, 0, Whence::End).map_err(|_| IO)?;
        if size > 2 {
            write_all(c.v, context, b",")?;
            copy(
                c.v,
                output,
                Span {
                    start: 1,
                    end: size - 1,
                },
                context,
            )?;
        }
        self.cursor = 1;
        self.ready = true;
        Ok(())
    }
    pub fn next_call(&mut self, v: &mut Vfs) -> Result<Option<Call>, &'static str> {
        if !self.ready {
            return Err("AI 工具轮次尚未完整校验");
        }
        if self.outstanding {
            return Err("AI 工具结果尚未提交");
        }
        let fd = self.output.ok_or(IO)?;
        let mut c = Cursor::new(v, fd, self.cursor);
        loop {
            c.whitespace()?;
            if c.peek()? == Some(b',') {
                c.byte()?;
                c.whitespace()?;
            }
            if c.peek()? == Some(b']') {
                return Ok(None);
            }
            c.expect(b'{')?;
            let (mut kind, mut name, mut id, mut arguments) = (None, None, None, None);
            loop {
                c.whitespace()?;
                if c.peek()? == Some(b'}') {
                    c.byte()?;
                    break;
                }
                let key = c.key()?;
                let span = c.span()?;
                let field = match key {
                    Key::Type => Some(&mut kind),
                    Key::Name => Some(&mut name),
                    Key::Call => Some(&mut id),
                    Key::Arguments => Some(&mut arguments),
                    Key::Other => None,
                };
                if let Some(field) = field {
                    if field.replace(span).is_some() {
                        return Err("AI 工具字段重复");
                    }
                }
                c.whitespace()?;
                match c.peek()? {
                    Some(b',') => {
                        c.byte()?;
                    }
                    Some(b'}') => (),
                    _ => return Err("AI 输出条目结构无效"),
                }
            }
            let next = c.at;
            self.cursor = next;
            let kind = kind.ok_or("AI 输出条目缺少类型")?;
            if !c.equals(kind, "function_call")? {
                c.at = next;
                continue;
            }
            let id = id.ok_or("AI 工具缺少调用标识")?;
            c.at = id.start;
            let mut count = 0usize;
            c.string(|b| {
                count += b.len();
                Ok(())
            })?;
            if count == 0 {
                return Err("AI 工具调用标识为空");
            }
            let name = name.ok_or("AI 工具缺少名称")?;
            let action = if c.equals(name, "device_status")? {
                Ok(0)
            } else if c.equals(name, "set_volume")? {
                Ok(1)
            } else if c.equals(name, "set_brightness")? {
                Ok(2)
            } else {
                Err("unknown_tool")
            };
            let action = action.and_then(|kind| {
                c.at = arguments.ok_or("invalid_arguments")?.start;
                let mut json = Json::arguments();
                c.string(|bytes| {
                    for b in bytes {
                        json.push(*b, &mut |_, _| Ok(true))?;
                    }
                    Ok(())
                })?;
                let e = json.finish().map_err(|_| "invalid_arguments")?;
                if e.invalid_arguments {
                    return Err("invalid_arguments");
                }
                if kind == 0 {
                    if e.argument_fields != 0 {
                        return Err("invalid_arguments");
                    }
                    return Ok(Action::Status);
                }
                let percent = e.percent.filter(|p| *p <= 100).ok_or("invalid_arguments")? as u8;
                if kind == 2 && percent < 10 {
                    return Err("brightness_requires_10_to_100");
                }
                Ok(if kind == 1 {
                    Action::Volume(percent)
                } else {
                    Action::Brightness(percent)
                })
            });
            self.outstanding = true;
            return Ok(Some(Call { action, id }));
        }
    }
    pub fn result(
        &mut self,
        v: &mut Vfs,
        call: Call,
        encoded_output: &[u8],
    ) -> Result<(), &'static str> {
        if !self.outstanding {
            return Err("AI 工具结果没有对应调用");
        }
        let fd = self.context.ok_or(IO)?;
        v.seek(fd, 0, Whence::End).map_err(|_| IO)?;
        write_all(v, fd, b",{\"type\":\"function_call_output\",\"call_id\":")?;
        copy(v, self.output.ok_or(IO)?, call.id, fd)?;
        write_all(v, fd, b",\"output\":")?;
        write_all(v, fd, encoded_output)?;
        write_all(v, fd, b"}")?;
        self.outstanding = false;
        Ok(())
    }
    pub fn close(mut self, v: &mut Vfs) -> Result<(), &'static str> {
        let mut failed = false;
        for fd in [self.context.take(), self.output.take()]
            .into_iter()
            .flatten()
        {
            failed |= v.close(fd).is_err();
        }
        for path in [CONTEXT, OUTPUT] {
            if v.unlink(path).is_err() {
                failed = true;
            }
        }
        if failed {
            Err(IO)
        } else {
            Ok(())
        }
    }
}
impl Drop for Agent {
    fn drop(&mut self) {
        for fd in [self.context.take(), self.output.take()]
            .into_iter()
            .flatten()
        {
            (self.defer_close)(fd);
        }
    }
}
fn copy(v: &mut Vfs, source: i32, span: Span, dest: i32) -> Result<(), &'static str> {
    v.seek(source, i64::from(span.start), Whence::Set)
        .map_err(|_| IO)?;
    let mut remaining = span.end - span.start;
    let mut bytes = [0; 1024];
    while remaining > 0 {
        let amount = bytes.len().min(remaining as usize);
        let n = v.read(source, &mut bytes[..amount]).map_err(|_| IO)?;
        if n == 0 {
            return Err(IO);
        }
        write_all(v, dest, &bytes[..n])?;
        remaining -= n as u32;
    }
    Ok(())
}
fn write_all(v: &mut Vfs, fd: i32, mut bytes: &[u8]) -> Result<(), &'static str> {
    while !bytes.is_empty() {
        let n = v.write(fd, bytes).map_err(|_| IO)?;
        if n == 0 {
            return Err(IO);
        }
        bytes = &bytes[n..];
    }
    Ok(())
}
