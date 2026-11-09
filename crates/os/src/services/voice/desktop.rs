//! USB attachment contract. The computer owns model/tool scheduling while its
//! lease is active; the device owns its controls and the current reply file.
use alloc::vec::Vec;
use voice_session::agent::Action;
pub const LEASE_MS: u64 = 10_000;
pub const ALL_CAPABILITIES: u8 = 7;
pub fn capability(action: Action) -> u8 {
    match action {
        Action::Status => 1,
        Action::Volume(_) => 2,
        Action::Brightness(_) => 4,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub nonce: u32,
    pub capabilities: u8,
    pub expires: u64,
}
impl Lease {
    pub fn new(nonce: u32, capabilities: u8, now: u64) -> Result<Self, &'static str> {
        if nonce == 0 || capabilities == 0 || capabilities & !ALL_CAPABILITIES != 0 {
            return Err("invalid agent lease");
        }
        Ok(Self {
            nonce,
            capabilities,
            expires: now.saturating_add(LEASE_MS),
        })
    }
    pub fn valid(self, nonce: u32, now: u64) -> bool {
        self.nonce == nonce && now < self.expires
    }
    pub fn renew(&mut self, nonce: u32, now: u64) -> Result<(), &'static str> {
        if !self.valid(nonce, now) {
            return Err("agent lease expired or replaced");
        }
        self.expires = now.saturating_add(LEASE_MS);
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Attach(u32, u8),
    Heartbeat(u32),
    Detach(u32),
    Poll(u32),
    Input(u32, u32, usize),
    Feed(u32, u32, usize, Vec<u8>),
    Tool(u32, u32, u32, Action),
    Result(u32, u32, u32, usize),
    Done(u32, u32, bool),
}
impl Command {
    pub fn parse(verb: &str, args: &[&str]) -> Result<Self, &'static str> {
        let number = |s: &str| s.parse::<u32>().map_err(|_| "invalid agent integer");
        let command = match (verb, args) {
            ("agent-attach", [n, c]) => Self::Attach(
                number(n)?,
                number(c)?.try_into().map_err(|_| "invalid capabilities")?,
            ),
            ("agent-heartbeat", [n]) => Self::Heartbeat(number(n)?),
            ("agent-detach", [n]) => Self::Detach(number(n)?),
            ("agent-poll", [n]) => Self::Poll(number(n)?),
            ("agent-input", [n, id, at]) => {
                Self::Input(number(n)?, number(id)?, number(at)? as usize)
            }
            ("agent-feed", [n, id, at, hex]) => Self::Feed(
                number(n)?,
                number(id)?,
                number(at)? as usize,
                crate::formats::hex::decode(hex, 100)?,
            ),
            ("agent-tool", [n, id, call, "status"]) => {
                Self::Tool(number(n)?, number(id)?, number(call)?, Action::Status)
            }
            ("agent-tool", [n, id, call, verb, percent]) => {
                let percent = number(percent)?;
                let action = match *verb {
                    "volume" if percent <= 100 => Action::Volume(percent as u8),
                    "brightness" if (10..=100).contains(&percent) => {
                        Action::Brightness(percent as u8)
                    }
                    _ => return Err("invalid agent device action"),
                };
                Self::Tool(number(n)?, number(id)?, number(call)?, action)
            }
            ("agent-result", [n, id, call, at]) => {
                Self::Result(number(n)?, number(id)?, number(call)?, number(at)? as usize)
            }
            ("agent-done", [n, id, state]) => Self::Done(
                number(n)?,
                number(id)?,
                match *state {
                    "ok" => true,
                    "error" => false,
                    _ => return Err("invalid agent outcome"),
                },
            ),
            _ => return Err("invalid agent command"),
        };
        Ok(command)
    }
}
#[cfg(target_os = "none")]
pub(crate) mod device;
