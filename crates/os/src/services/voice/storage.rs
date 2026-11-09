//! The services executor owns NAND sessions and serializes their lifecycle.
#[cfg(target_os = "none")]
use alloc::{string::String, vec::Vec};
#[cfg(target_os = "none")]
use fs::Vfs;
pub use voice_session::storage::{chunk, finish, window, Session, Window, PATH};
#[cfg(target_os = "none")]
const IO: &str = "语音记录存储失败，请检查 NAND 空间";
#[cfg(target_os = "none")]
pub(crate) mod device {
    use super::super::device as voice;
    use super::*;
    use alloc::boxed::Box;
    use core::cell::RefCell;
    use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
    static CURRENT: Mutex<CriticalSectionRawMutex, RefCell<Option<Box<Session>>>> =
        Mutex::new(RefCell::new(None));
    pub enum Job {
        Open(u32, String),
        PlainOpen(u32),
        PlainFeed(u32, usize, Vec<u8>),
        Feed(u32, Vec<u8>, usize),
        Page(u32, usize),
        Read(usize),
        Finish(u32, super::super::Phase),
        ContextBytes,
        ContextRead(u32, Vec<u8>),
        NextTool,
        ToolResult(voice_session::agent::Call, String),
        NextRound,
    }
    pub enum Output {
        Empty,
        Block(Vec<u8>, bool),
        Text(String),
        Number(u32),
        Bytes(Vec<u8>, usize),
        Tool(Option<voice_session::agent::Call>),
    }
    // All storage jobs share one executor/result type. CURRENT is taken only
    // briefly under the IRQ lock; filesystem and parsing run with IRQs enabled.
    pub async fn run(job: Job) -> Result<Output, &'static str> {
        crate::platform::runtime::executors::services::run(move || {
            let mut current = CURRENT.lock(|s| s.borrow_mut().take());
            let result = fs::global::with(|v| Ok(execute(v, &mut current, job)))
                .map_err(|_| "语音记录存储不可用")
                .and_then(|r| r);
            CURRENT.lock(|s| *s.borrow_mut() = current);
            result
        })
        .await
    }
    fn execute(
        v: &mut Vfs,
        current: &mut Option<Box<Session>>,
        job: Job,
    ) -> Result<Output, &'static str> {
        match job {
            Job::PlainOpen(id) => {
                if current.is_some() {
                    return Err("AI 记录仍在使用");
                }
                *current = Some(Box::new(Session::new(
                    v,
                    crate::platform::runtime::executors::services::close_later,
                )?));
                voice::archive(id, PATH.into());
                Ok(Output::Empty)
            }
            Job::PlainFeed(id, offset, bytes) => {
                let session = current.as_mut().ok_or(IO)?;
                session.append_plain(v, offset, &bytes)?;
                voice::stored(id, session.total(), session.window()?);
                Ok(Output::Number(session.total() as u32))
            }
            Job::Open(id, text) => {
                if current.is_some() {
                    return Err("语音记录仍在使用");
                }
                let mut session = Box::new(Session::new(
                    v,
                    crate::platform::runtime::executors::services::close_later,
                )?);
                let user = alloc::format!(
                    "{{\"type\":\"message\",\"role\":\"user\",\"content\":{}}}",
                    super::super::json_string(&text)
                );
                if let Err(error) = session.enable_agent(v, user.as_bytes()) {
                    let _ = session.close(v);
                    return Err(error);
                }
                voice::archive(id, PATH.into());
                *current = Some(session);
                Ok(Output::Empty)
            }
            Job::ContextBytes => Ok(Output::Number(
                current.as_mut().ok_or(IO)?.agent()?.context_bytes(v)?,
            )),
            Job::ContextRead(offset, mut bytes) => {
                let n = current
                    .as_mut()
                    .ok_or(IO)?
                    .agent()?
                    .read_context(v, offset, &mut bytes)?;
                Ok(Output::Bytes(bytes, n))
            }
            Job::NextTool => Ok(Output::Tool(
                current.as_mut().ok_or(IO)?.agent()?.next_call(v)?,
            )),
            Job::ToolResult(call, result) => {
                let encoded = super::super::json_string(&result);
                current
                    .as_mut()
                    .ok_or(IO)?
                    .agent()?
                    .result(v, call, encoded.as_bytes())?;
                Ok(Output::Empty)
            }
            Job::NextRound => {
                current.as_mut().ok_or(IO)?.next_round()?;
                Ok(Output::Empty)
            }
            Job::Feed(id, bytes, n) => {
                let session = current.as_mut().ok_or(IO)?;
                let result = session.feed(v, &bytes[..n]);
                if result != Ok(false) {
                    let total = session.total();
                    voice::stored(id, total, session.window()?);
                }
                result?;
                Ok(Output::Block(bytes, session.reply.done))
            }
            Job::Page(id, offset) => {
                let (active, path, total) = voice::record();
                if active == id {
                    voice::stored_window(id, window(v, &path, offset, total)?);
                }
                Ok(Output::Empty)
            }
            Job::Read(offset) => {
                let (id, path, total) = voice::record();
                let raw = if path.is_empty() && offset == 0 {
                    Vec::new()
                } else {
                    chunk(v, &path, offset, total)?
                };
                let hex = crate::formats::hex::encode(&raw);
                Ok(Output::Text(super::super::reply_record(
                    id, offset, total, &hex,
                )))
            }
            Job::Finish(id, phase) => {
                if let Some(session) = current.take() {
                    let total = session.total();
                    voice::stored(id, total, session.window()?);
                    session.close(v)?;
                    finish(v, PATH, total, phase.name())?;
                }
                Ok(Output::Empty)
            }
        }
    }
}
