//! The audio producer owns the closing fence. The voice consumer only requests
//! finish/abort and drains the reusable queue; neither side owns the other's IO.
use crate::services::voice::capture::Buffer;
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};

static BUFFER: Mutex<CriticalSectionRawMutex, RefCell<Option<Buffer>>> =
    Mutex::new(RefCell::new(None));
#[derive(Clone, Copy)]
pub struct Status {
    pub ready: bool,
    pub finish: bool,
    pub closed: bool,
    pub drained: bool,
    pub ms: u32,
    pub bytes: usize,
    pub error: Option<&'static str>,
}
pub async fn initialize() -> Result<(), &'static str> {
    if BUFFER.lock(|b| b.borrow().is_some()) {
        return Ok(());
    }
    let buffer = crate::platform::runtime::executors::services::run(Buffer::new).await?;
    BUFFER.lock(|b| *b.borrow_mut() = Some(buffer));
    Ok(())
}
pub fn start(id: u32) -> Result<(), &'static str> {
    BUFFER.lock(|b| b.borrow_mut().as_mut().ok_or("语音缓冲未就绪")?.start(id))
}
pub fn ready(id: u32) {
    BUFFER.lock(|b| {
        if let Some(b) = b.borrow_mut().as_mut() {
            b.mark_ready(id);
        }
    });
}
pub fn finish(id: u32) {
    BUFFER.lock(|b| {
        if let Some(b) = b.borrow_mut().as_mut() {
            b.request_finish(id);
        }
    });
}
pub fn abort(id: u32) {
    BUFFER.lock(|b| {
        if let Some(b) = b.borrow_mut().as_mut() {
            b.abort(id);
        }
    });
}
pub fn close(id: u32, error: Option<&'static str>) {
    BUFFER.lock(|b| {
        if let Some(b) = b.borrow_mut().as_mut() {
            b.close(id, error);
        }
    });
}
pub fn push(id: u32, pcm: &[u8]) -> Result<(), &'static str> {
    BUFFER.lock(|b| {
        b.borrow_mut()
            .as_mut()
            .ok_or("语音缓冲未就绪")?
            .push(id, pcm)
    })
}
pub fn pop(id: u32, out: &mut [u8]) -> Result<usize, &'static str> {
    BUFFER.lock(|b| {
        b.borrow_mut()
            .as_mut()
            .ok_or("语音缓冲未就绪")?
            .pop(id, out)
    })
}
pub fn status(id: u32) -> Result<Status, &'static str> {
    BUFFER.lock(|b| {
        let guard = b.borrow();
        let b = guard
            .as_ref()
            .filter(|b| b.accepts(id))
            .ok_or("过期的麦克风会话")?;
        Ok(Status {
            ready: b.ready,
            finish: b.finish,
            closed: b.closed,
            drained: b.drained(id),
            ms: b.milliseconds(),
            bytes: b.queued(),
            error: b.error,
        })
    })
}
