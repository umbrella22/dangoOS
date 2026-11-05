use alloc::{string::String, vec::Vec};
pub const COMMAND_BYTES_MAX: usize = 2 * 1024 * 1024;
/// One bounded request; file IO and pixel payloads use separate requests.
pub const BATCH_COMMANDS_MAX: usize = 64;
pub const BATCH_BYTES_MAX: usize = 16 * 1024;
pub const BATCH_OBJECTS_MAX: usize = 16;
/// Scheduled execution per UI owner slice, checked between native commands.
/// Exhaustion yields a continuation; it does not fail the request.
pub const BATCH_SLICE_US: u64 = 4_000;
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
