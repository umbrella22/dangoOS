//! Complete UTF-8 replies on NAND; RAM holds a write block and a reading window.
use crate::{
    sse::{Framer, Token},
    Reply, WINDOW_BYTES,
};
use alloc::{format, string::String, vec, vec::Vec};
use fs::{OpenFlags, Vfs, Whence};
const IO: &str = "语音记录存储失败，请检查 NAND 空间";
pub const PATH: &str = "/data/voice/current.txt";
// One logical NAND filesystem block between durable checkpoints. This batches
// tiny provider deltas without bounding the length of a reply.
const CHECKPOINT_BYTES: usize = 4096;

pub struct Window {
    pub start: usize,
    pub text: String,
}
pub struct Session {
    fd: Option<i32>,
    defer_close: fn(i32),
    pub reply: Reply,
    json: crate::json_stream::Json,
    framing: Framer,
    pending: Vec<u8>,
    committed: usize,
    synced: usize,
    candidate_tail: Vec<u8>,
    committed_tail: Vec<u8>,
    comparison: Vec<u8>,
    comparison_start: usize,
    comparison_end: usize,
    agent: Option<crate::agent::Agent>,
}
impl Session {
    #[inline(never)]
    pub fn new(v: &mut Vfs, defer_close: fn(i32)) -> Result<Self, &'static str> {
        // This directory belongs to the current turn. Replacing it also clears
        // records from the earlier per-turn layout; credentials live elsewhere.
        match v.remove_tree("/data/voice") {
            Ok(()) | Err(fs::Errno::NOENT) => (),
            Err(_) => return Err(IO),
        }
        v.create_dir_all("/data/voice").map_err(|_| IO)?;
        let fd = v
            .open(
                PATH,
                OpenFlags {
                    read: true,
                    exclusive: true,
                    ..OpenFlags::WRITE
                },
            )
            .map_err(|_| IO)?;
        if v.write_file(&format!("{PATH}.state"), b"replying\n")
            .is_err()
        {
            let _ = v.close(fd);
            return Err(IO);
        }
        let reply = Reply::default();
        let json = reply.json();
        Ok(Self {
            fd: Some(fd),
            defer_close,
            reply,
            json,
            framing: Framer::default(),
            pending: Vec::with_capacity(1024),
            committed: 0,
            synced: 0,
            candidate_tail: Vec::with_capacity(WINDOW_BYTES),
            committed_tail: Vec::with_capacity(WINDOW_BYTES),
            comparison: vec![0; 1024],
            comparison_start: 0,
            comparison_end: 0,
            agent: None,
        })
    }
    pub fn enable_agent(&mut self, v: &mut Vfs, user_json: &[u8]) -> Result<(), &'static str> {
        if self.agent.is_some() {
            return Err("AI 任务已创建");
        }
        self.agent = Some(crate::agent::Agent::new(v, self.defer_close, user_json)?);
        Ok(())
    }
    pub fn agent(&mut self) -> Result<&mut crate::agent::Agent, &'static str> {
        self.agent.as_mut().ok_or("AI 任务上下文不可用")
    }
    pub fn next_round(&mut self) -> Result<(), &'static str> {
        self.reply.next_round()?;
        self.json = self.reply.json();
        self.framing = Framer::default();
        Ok(())
    }
    fn rollback(&mut self, v: &mut Vfs) -> Result<(), &'static str> {
        self.pending.clear();
        self.candidate_tail.clone_from(&self.committed_tail);
        let fd = self.fd.unwrap();
        v.ftruncate(fd, self.committed as u32).map_err(|_| IO)?;
        v.seek(fd, self.committed as i64, Whence::Set)
            .map_err(|_| IO)?;
        Ok(())
    }
    #[inline(never)]
    pub fn feed(&mut self, v: &mut Vfs, bytes: &[u8]) -> Result<bool, &'static str> {
        let result = self.feed_inner(v, bytes);
        if result.is_err() {
            let _ = self.rollback(v);
        }
        result
    }
    #[inline(never)]
    fn feed_inner(&mut self, v: &mut Vfs, bytes: &[u8]) -> Result<bool, &'static str> {
        let mut changed = false;
        let fd = self.fd.unwrap();
        for b in bytes {
            match self.framing.push(*b) {
                Some(Token::Byte(b)) => {
                    let pending = &mut self.pending;
                    let comparison = &mut self.comparison;
                    let cache_start = &mut self.comparison_start;
                    let cache_end = &mut self.comparison_end;
                    let total = self.committed;
                    let tail = &mut self.candidate_tail;
                    let agent = &mut self.agent;
                    self.json.push(b, &mut |source, raw| match source {
                        crate::json_stream::TextSource::Append => {
                            if pending.len() + raw.len() > 1024 {
                                write_all(v, fd, pending)?;
                                pending.clear();
                            }
                            pending.extend_from_slice(raw);
                            let discard = (tail.len() + raw.len()).saturating_sub(WINDOW_BYTES);
                            tail.copy_within(discard.., 0);
                            tail.truncate(tail.len() - discard);
                            tail.extend_from_slice(raw);
                            Ok(true)
                        }
                        crate::json_stream::TextSource::Compare(offset) => {
                            if offset.saturating_add(raw.len()) > total {
                                return Ok(false);
                            }
                            for (index, byte) in raw.iter().enumerate() {
                                let at = offset + index;
                                if at < *cache_start || at >= *cache_end {
                                    let position = v.seek(fd, 0, Whence::Cur).map_err(|_| IO)?;
                                    v.seek(fd, at as i64, Whence::Set).map_err(|_| IO)?;
                                    let amount = comparison.len().min(total - at);
                                    let n =
                                        v.read(fd, &mut comparison[..amount]).map_err(|_| IO)?;
                                    v.seek(fd, i64::from(position), Whence::Set)
                                        .map_err(|_| IO)?;
                                    if n == 0 {
                                        return Err(IO);
                                    }
                                    *cache_start = at;
                                    *cache_end = at + n;
                                }
                                if comparison[at - *cache_start] != *byte {
                                    return Ok(false);
                                }
                            }
                            Ok(true)
                        }
                        source @ (crate::json_stream::TextSource::OutputStart
                        | crate::json_stream::TextSource::Output) => match agent {
                            Some(agent) => agent.archive(v, source, raw),
                            None => Ok(true),
                        },
                    })?;
                }
                Some(Token::End) => {
                    let event = core::mem::take(&mut self.json).finish()?;
                    if self.reply.accept(event)? {
                        write_all(v, fd, &self.pending)?;
                        self.pending.clear();
                        self.committed = self.reply.bytes();
                        self.committed_tail.clone_from(&self.candidate_tail);
                        changed = true;
                    } else if !self.pending.is_empty()
                        || v.fstat(fd).map_err(|_| IO)?.size as usize != self.committed
                    {
                        self.rollback(v)?;
                    }
                    self.json = self.reply.json();
                    if self.reply.done {
                        if let Some(agent) = &mut self.agent {
                            agent.commit_round(v)?;
                        }
                        break;
                    }
                }
                None => (),
            }
        }
        if changed && self.committed.saturating_sub(self.synced) >= CHECKPOINT_BYTES {
            v.fsync(fd).map_err(|_| IO)?;
            self.synced = self.committed;
        }
        Ok(changed)
    }
    #[cfg(not(target_os = "none"))]
    pub fn buffered_bytes(&self) -> usize {
        self.pending.capacity()
            + self.comparison.capacity()
            + self.candidate_tail.capacity()
            + self.committed_tail.capacity()
            + self.json.buffered_bytes()
            + if self.agent.is_some() { 1024 } else { 0 }
    }
    /// A trusted desktop executor supplies visible UTF-8 chunks at explicit byte
    /// offsets. The provider context stays on the computer; this file retains
    /// only the current task's complete visible reply.
    pub fn append_plain(
        &mut self,
        v: &mut Vfs,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), &'static str> {
        if self.agent.is_some() || offset != self.committed {
            return Err("AI 回复偏移或执行模式不匹配");
        }
        let text = core::str::from_utf8(bytes).map_err(|_| "AI 回复不是完整 UTF-8")?;
        if text.contains('\0') {
            return Err("AI 回复包含无效文字");
        }
        let total = self
            .committed
            .checked_add(bytes.len())
            .filter(|n| *n <= u32::MAX as usize)
            .ok_or("AI 回复超出文件可表示范围")?;
        let fd = self.fd.ok_or(IO)?;
        if let Err(error) = write_all(v, fd, bytes) {
            let _ = self.rollback(v);
            return Err(error);
        }
        // Keep the existing fixed reading window, including when a caller's
        // single chunk is larger than it. No whole-reply allocation occurs.
        let bytes = &bytes[bytes.len().saturating_sub(WINDOW_BYTES)..];
        let discard = (self.committed_tail.len() + bytes.len()).saturating_sub(WINDOW_BYTES);
        self.committed_tail.drain(..discard);
        self.committed_tail.extend_from_slice(bytes);
        self.candidate_tail.clone_from(&self.committed_tail);
        self.committed = total;
        if self.committed.saturating_sub(self.synced) >= CHECKPOINT_BYTES {
            v.fsync(fd).map_err(|_| IO)?;
            self.synced = self.committed;
        }
        Ok(())
    }
    pub fn total(&self) -> usize {
        self.committed
    }
    #[inline(never)]
    pub fn window(&self) -> Result<Window, &'static str> {
        // Reading the active writer flushes LittleFS's append cache and makes
        // the next tiny delta copy its last block. Keep the accepted tail in
        // bounded RAM instead; provisional event bytes never reach this view.
        let raw = &self.committed_tail;
        let skip = raw.iter().take_while(|b| **b & 0xc0 == 0x80).count();
        let text = core::str::from_utf8(&raw[skip..]).map_err(|_| "语音记录文字损坏")?;
        Ok(Window {
            start: self.committed - raw.len() + skip,
            text: text.into(),
        })
    }
    #[inline(never)]
    pub fn close(mut self, v: &mut Vfs) -> Result<(), &'static str> {
        let result = self.rollback(v);
        let closed = v.close(self.fd.take().unwrap());
        let context = self
            .agent
            .take()
            .map(|agent| agent.close(v))
            .unwrap_or(Ok(()));
        result?;
        closed.map_err(|_| IO)?;
        context
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            (self.defer_close)(fd);
        }
    }
}
fn write_all(v: &mut Vfs, fd: i32, bytes: &[u8]) -> Result<(), &'static str> {
    let mut at = 0;
    while at < bytes.len() {
        let n = v.write(fd, &bytes[at..]).map_err(|_| IO)?;
        if n == 0 {
            return Err(IO);
        }
        at += n;
    }
    Ok(())
}
#[inline(never)]
fn read_fd(
    v: &mut Vfs,
    fd: i32,
    start: usize,
    total: usize,
    amount: usize,
) -> Result<Window, &'static str> {
    if start > total {
        return Err("语音页码无效");
    }
    v.seek(fd, start as i64, Whence::Set).map_err(|_| IO)?;
    let mut raw = vec![0; amount.min(total - start)];
    let mut at = 0;
    while at < raw.len() {
        let n = v.read(fd, &mut raw[at..]).map_err(|_| IO)?;
        if n == 0 {
            return Err(IO);
        }
        at += n;
    }
    let skip = raw.iter().take_while(|b| **b & 0xc0 == 0x80).count();
    if skip > 3 {
        return Err("语音记录文字损坏");
    }
    let end = match core::str::from_utf8(&raw[skip..]) {
        Ok(_) => raw.len(),
        Err(e) if e.error_len().is_none() => skip + e.valid_up_to(),
        Err(_) => return Err("语音记录文字损坏"),
    };
    Ok(Window {
        start: start + skip,
        text: core::str::from_utf8(&raw[skip..end])
            .map_err(|_| "语音记录文字损坏")?
            .into(),
    })
}
#[inline(never)]
pub fn window(v: &mut Vfs, path: &str, start: usize, total: usize) -> Result<Window, &'static str> {
    let fd = v.open(path, OpenFlags::READ).map_err(|_| IO)?;
    let result = read_fd(v, fd, start, total, WINDOW_BYTES);
    let closed = v.close(fd);
    let result = result?;
    closed.map_err(|_| IO)?;
    Ok(result)
}
#[inline(never)]
pub fn chunk(
    v: &mut Vfs,
    path: &str,
    offset: usize,
    total: usize,
) -> Result<Vec<u8>, &'static str> {
    if offset > total {
        return Err("voice reply offset out of range");
    }
    let fd = v.open(path, OpenFlags::READ).map_err(|_| IO)?;
    let result = (|| {
        v.seek(fd, offset as i64, Whence::Set).map_err(|_| IO)?;
        let mut bytes = [0; 40];
        let mut n = 0;
        let end = 40.min(total - offset);
        while n < end {
            let count = v.read(fd, &mut bytes[n..end]).map_err(|_| IO)?;
            if count == 0 {
                return Err(IO);
            }
            n += count;
        }
        Ok(bytes[..n].to_vec())
    })();
    let closed = v.close(fd);
    let result = result?;
    closed.map_err(|_| IO)?;
    Ok(result)
}
#[inline(never)]
pub fn finish(v: &mut Vfs, path: &str, total: usize, state: &str) -> Result<(), &'static str> {
    let fd = v
        .open(
            path,
            OpenFlags {
                read: true,
                write: true,
                ..OpenFlags::default()
            },
        )
        .map_err(|_| IO)?;
    let result = v.ftruncate(fd, total as u32).and_then(|_| v.fsync(fd));
    let closed = v.close(fd);
    result.map_err(|_| IO)?;
    closed.map_err(|_| IO)?;
    v.write_file(&format!("{path}.state"), state.as_bytes())
        .map_err(|_| IO)
}
