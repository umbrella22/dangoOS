//! Typed ownership boundary: the embedding executes commands on its UI owner.
use crate::{Call, Error, Request, Ret, Vm};
use alloc::{string::String, vec::Vec};
pub const REQUEST: u32 = 0x4C560001;
pub const COMMAND_BYTES_MAX: usize = 2 * 1024 * 1024;
#[derive(Debug)]
pub struct UiCommand {
    pub op: String,
    pub args: String,
    pub bytes: Vec<u8>,
    pub root: String,
}
#[derive(Debug)]
pub struct UiReply {
    pub json: String,
    pub bytes: Vec<u8>,
}
impl UiReply {
    pub fn json(json: String) -> Self {
        Self {
            json,
            bytes: Vec::new(),
        }
    }
}
pub(crate) fn register(vm: &mut Vm) -> Result<(), String> {
    vm.register("_lvgl", &[("request", request), ("reply", reply)])
}
fn request(c: &mut Call) -> Result<Ret, Error> {
    if !c.ctx.ui_allowed {
        return Err(Error::msg("workers cannot acquire LVGL"));
    }
    if !c.can_yield() {
        return Err(Error::msg("LVGL requires a yieldable foreground callback"));
    }
    let op: String = c.str(1)?.into();
    if op == "init" && c.ctx.host.display_info().is_some() {
        return Err(Error::msg("display already owned"));
    }
    let args = c.str(2)?.into();
    let bytes = c.bytes(3)?;
    if bytes.len() > COMMAND_BYTES_MAX {
        return Err(Error::msg("LVGL pixel upload exceeds 2 MiB"));
    }
    let bytes = bytes.into();
    let root = String::from_utf8_lossy(&c.ctx.data_root[..c.ctx.data_root.len() - 1]).into_owned();
    c.ctx.ui_waiting = Some(op.clone());
    c.ctx.ui_command = Some(UiCommand {
        op,
        args,
        bytes,
        root,
    });
    Ok(Ret::Yield(Request::Host(REQUEST)))
}
fn reply(c: &mut Call) -> Result<Ret, Error> {
    match c
        .ctx
        .ui_reply
        .take()
        .ok_or_else(|| Error::msg("missing UI completion"))?
    {
        Ok(reply) => {
            c.push_str(&reply.json);
            c.push_bytes(&reply.bytes);
            Ok(Ret::Values(2))
        }
        Err(error) => Err(Error::msg(error)),
    }
}
impl Vm {
    pub fn take_ui_command(&mut self) -> Option<UiCommand> {
        unsafe { (*self.ctx).ui_command.take() }
    }
    pub fn complete_ui(&mut self, reply: Result<UiReply, String>) {
        unsafe {
            let ctx = &mut *self.ctx;
            if reply.is_ok() {
                match ctx.ui_waiting.take().as_deref() {
                    Some("init") => ctx.ui_owned = true,
                    Some("deinit") => ctx.ui_owned = false,
                    _ => {}
                }
            }
            ctx.ui_reply = Some(reply);
        }
    }
    pub fn disable_ui(&mut self) {
        unsafe {
            (*self.ctx).ui_allowed = false;
        }
    }
}
