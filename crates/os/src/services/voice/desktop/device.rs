use super::{capability, Command, Lease};
use crate::services::voice::{
    device as voice,
    storage::device::{run as storage, Job},
};
use alloc::{format, string::String};
use core::cell::RefCell;
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_time::{Instant, Timer};
use voice_session::agent::Action;
struct Task {
    id: u32,
    text: String,
    bytes: usize,
    outcome: Option<bool>,
    last: Option<(u32, Action, String)>,
}
struct State {
    lease: Option<Lease>,
    task: Option<Task>,
}
static STATE: Mutex<CriticalSectionRawMutex, RefCell<State>> = Mutex::new(RefCell::new(State {
    lease: None,
    task: None,
}));
fn now() -> u64 {
    Instant::now().as_millis()
}
pub(crate) fn attached() -> Option<u32> {
    STATE.lock(|s| {
        s.borrow()
            .lease
            .filter(|l| l.valid(l.nonce, now()))
            .map(|l| l.nonce)
    })
}
fn check(s: &mut State, nonce: u32) -> Result<Lease, &'static str> {
    s.lease
        .filter(|l| l.valid(nonce, now()))
        .ok_or("agent lease expired or replaced")
}
fn task(s: &mut State, nonce: u32, id: u32) -> Result<&mut Task, &'static str> {
    check(s, nonce)?;
    s.task
        .as_mut()
        .filter(|t| t.id == id && t.outcome.is_none())
        .ok_or("agent task cancelled or replaced")
}
fn alive(nonce: u32, id: u32) -> bool {
    STATE.lock(|s| task(&mut s.borrow_mut(), nonce, id).is_ok())
}
struct Ticket(u32);
impl Drop for Ticket {
    fn drop(&mut self) {
        STATE.lock(|s| {
            let mut s = s.borrow_mut();
            if s.task.as_ref().is_some_and(|t| t.id == self.0) {
                s.task = None;
            }
        });
    }
}
pub(crate) async fn answer(nonce: u32, id: u32, text: String) -> Result<(), &'static str> {
    storage(Job::PlainOpen(id)).await?;
    STATE.lock(|s| {
        let mut s = s.borrow_mut();
        check(&mut s, nonce)?;
        if s.task.is_some() {
            return Err("agent task already active");
        }
        s.task = Some(Task {
            id,
            text,
            bytes: 0,
            outcome: None,
            last: None,
        });
        Ok(())
    })?;
    let _ticket = Ticket(id);
    loop {
        let outcome = STATE.lock(|s| {
            let mut s = s.borrow_mut();
            check(&mut s, nonce)?;
            s.task
                .as_ref()
                .filter(|t| t.id == id)
                .map(|t| t.outcome)
                .ok_or("agent task cancelled or replaced")
        })?;
        match outcome {
            Some(true) => return Ok(()),
            Some(false) => return Err("电脑端 AI 任务失败"),
            None => (),
        }
        Timer::after_millis(20).await;
    }
}
async fn cancelled(nonce: u32, id: u32) {
    while alive(nonce, id) {
        Timer::after_millis(20).await;
    }
}
pub(crate) async fn execute(command: Command) -> String {
    match execute_inner(command).await {
        Ok(line) => format!("@ok {line}"),
        Err(e) => format!("@err {e}"),
    }
}
async fn execute_inner(command: Command) -> Result<String, &'static str> {
    match command {
        Command::Attach(n, caps) => {
            STATE.lock(|s| {
                let mut s = s.borrow_mut();
                if voice::busy()
                    || s.lease
                        .is_some_and(|l| l.valid(l.nonce, now()) && l.nonce != n)
                {
                    return Err("agent device already in use");
                }
                s.lease = Some(Lease::new(n, caps, now())?);
                s.task = None;
                Ok(())
            })?;
            Ok(format!("agent-attach protocol=1 lease={n} caps={caps}"))
        }
        Command::Heartbeat(n) => {
            STATE.lock(|s| {
                s.borrow_mut()
                    .lease
                    .as_mut()
                    .ok_or("agent not attached")?
                    .renew(n, now())
            })?;
            Ok("agent-heartbeat".into())
        }
        Command::Detach(n) => {
            STATE.lock(|s| {
                let mut s = s.borrow_mut();
                check(&mut s, n)?;
                s.lease = None;
                Ok(())
            })?;
            Ok("agent-detach".into())
        }
        Command::Poll(n) => STATE.lock(|s| {
            let mut s = s.borrow_mut();
            check(&mut s, n)?;
            Ok(match &s.task {
                Some(t) => format!(
                    "agent-poll id={} bytes={} output={}",
                    t.id,
                    t.text.len(),
                    t.bytes
                ),
                None => "agent-poll id=0 bytes=0 output=0".into(),
            })
        }),
        Command::Input(n, id, at) => STATE.lock(|s| {
            let mut s = s.borrow_mut();
            let t = task(&mut s, n, id)?;
            let bytes = t.text.as_bytes();
            let end = at.saturating_add(40).min(bytes.len());
            let chunk = bytes.get(at..end).ok_or("agent input offset")?;
            Ok(format!(
                "agent-input hex={}",
                crate::formats::hex::encode(chunk)
            ))
        }),
        Command::Feed(n, id, at, bytes) => {
            STATE.lock(|s| {
                let mut s = s.borrow_mut();
                let t = task(&mut s, n, id)?;
                if at != t.bytes {
                    return Err("agent reply offset mismatch");
                }
                Ok(())
            })?;
            let total = at
                .checked_add(bytes.len())
                .ok_or("agent reply offset overflow")?;
            storage(Job::PlainFeed(id, at, bytes)).await?;
            STATE.lock(|s| {
                let mut s = s.borrow_mut();
                task(&mut s, n, id)?.bytes = total;
                Ok(())
            })?;
            Ok(format!("agent-feed bytes={total}"))
        }
        Command::Tool(n, id, index, action) => {
            let cached = STATE.lock(|s| {
                let mut s = s.borrow_mut();
                let lease = check(&mut s, n)?;
                if lease.capabilities & capability(action) == 0 {
                    return Err("agent capability denied");
                }
                let t = task(&mut s, n, id)?;
                let last = t.last.as_ref().map_or(0, |x| x.0);
                if index == last && t.last.as_ref().is_some_and(|x| x.1 == action) {
                    return Ok(t.last.as_ref().map(|x| x.2.len()));
                }
                if index != last.checked_add(1).ok_or("agent call sequence exhausted")? {
                    return Err("agent call sequence mismatch");
                }
                Ok(None)
            })?;
            let count = if let Some(count) = cached {
                count
            } else {
                let result = match embassy_futures::select::select(
                    crate::apps::agent::execute(Ok(action)),
                    cancelled(n, id),
                )
                .await
                {
                    embassy_futures::select::Either::First(result) => result,
                    embassy_futures::select::Either::Second(()) => {
                        return Err("agent task cancelled")
                    }
                };
                let count = result.len();
                STATE.lock(|s| {
                    let mut s = s.borrow_mut();
                    task(&mut s, n, id)?.last = Some((index, action, result));
                    Ok(())
                })?;
                count
            };
            Ok(format!("agent-tool call={index} bytes={count}"))
        }
        Command::Result(n, id, index, at) => STATE.lock(|s| {
            let mut s = s.borrow_mut();
            let t = task(&mut s, n, id)?;
            let (_, _, result) = t
                .last
                .as_ref()
                .filter(|x| x.0 == index)
                .ok_or("agent result not available")?;
            let end = at.saturating_add(40).min(result.len());
            let bytes = result
                .as_bytes()
                .get(at..end)
                .ok_or("agent result offset")?;
            Ok(format!(
                "agent-result hex={}",
                crate::formats::hex::encode(bytes)
            ))
        }),
        Command::Done(n, id, ok) => {
            STATE.lock(|s| {
                let mut s = s.borrow_mut();
                task(&mut s, n, id)?.outcome = Some(ok);
                Ok(())
            })?;
            Ok("agent-done".into())
        }
    }
}
