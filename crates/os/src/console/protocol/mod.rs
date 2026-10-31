//! The USB app push protocol, independent of the USB stack.
//!
//! The console port carries log lines out and this protocol in. Lines that
//! do not start with `@mosaico ` are ignored, so typing into a terminal is
//! harmless. Every command gets one reply line, `@ok ...` or `@err ...`.
//!
//! ```text
//! @mosaico hello                       -> @ok hello mosaico-push 1
//! @mosaico begin <id>                  -> @ok begin <id>
//! @mosaico file <rel> <size> <crc32>   -> @ok ready <rel>
//!   (after `ready`, exactly <size> raw bytes)  -> @ok file <rel>
//! @mosaico commit <id> [run]           -> @ok commit <id>
//! @mosaico abort                       -> @ok abort
//! @mosaico run <id> | stop | reload    -> @ok ...
//! @mosaico rm <id>                     -> @ok rm <id>
//! @mosaico ls                          -> @ok ls <id> <id> ...
//! @mosaico status                      -> @ok status <mount>:<free>/<blocks>x<size> ...
//! @mosaico reboot                      -> @ok reboot (then a software reset)
//! @mosaico nand-info                   -> @ok nand-info id=... (read only)
//! @mosaico screen-off <1..60> | screen-on | power-status
//! ```
//!
//! `begin` stages a fresh `/data/apps/.incoming/<id>`; `file` writes into it
//! (the CRC-32 is the zlib one, in hex); `commit` checks the manifest, swaps
//! the staged directory for `/data/apps/<id>` and asks for a rescan. Apps in
//! `/system` cannot be changed, but a `/data` app with the same id shadows
//! one, and `rm` brings it back.

mod commands;
mod framing;
mod ota;
#[cfg(test)]
mod tests;
mod upload;

use ::fs::{Errno, Vfs};
use alloc::{string::String, vec::Vec};
pub use upload::rel_is_safe;

pub const PREFIX: &str = "@mosaico ";
pub const VERSION: u32 = 1;
const DATA_APPS: &str = "/data/apps";
const STAGING: &str = "/data/apps/.incoming";
const PHOTO_STAGING: &str = "/data/appdata/photo/.incoming";
const PHOTO_ROOT: &str = "/data/appdata/photo/photos";
/// Longest command line; longer ones are discarded whole.
// Includes the console prefix and a full update URL or encoded Wi-Fi credentials.
const LINE_MAX: usize = 320;
/// Largest single file.
pub const FILE_MAX: u32 = 1024 * 1024;
/// Deepest relative path, in components.
const DEPTH_MAX: usize = 6;

/// What the firmware should do after a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Cancel a transfer silently while its malformed binary body is drained.
    OtaCancel,
    /// One reply line, without the line ending.
    Reply(String),
    /// Rescan the apps, then open `run` if given.
    Reload {
        run: Option<String>,
    },
    Run(String),
    Manage(crate::apps::lifecycle::uninstall::Operation),
    Stop,
    /// Reset the chip once the reply is out.
    Reboot,
    /// Reload the persisted skin selection on the UI task.
    SkinsChanged,
    /// The UI task validates/commits the resident brain and returns the reply.
    Brain(crate::companion::brain::store::Action),
    Ota(crate::services::update::ota::Command),
    /// Read-only device discovery, executed after releasing the VFS lock.
    NandInfo,

    NandStatus,
    NandFormat,

    Audio(crate::services::audio::Command),
    Radio(crate::services::radio::Command),
    Update(crate::services::update::Command),
    Camera(crate::services::camera::Command),
    Power(crate::services::power::standby::Command),
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum UploadKind {
    #[default]
    App,
    Skin,
    Brain,
    Photo,
}

struct Upload {
    fd: i32,
    rel: String,
    path: String,
    left: u32,
    crc: u32,
    want: u32,
    failed: Option<Errno>,
}

struct OtaBody {
    offset: u32,
    len: usize,
    crc: u32,
    bytes: Vec<u8>,
    invalid: bool,
}

/// One connection's protocol state.
#[derive(Default)]
pub struct Session {
    ota_body: Option<OtaBody>,
    ota_active: bool,
    ota_poisoned: bool,
    line: Vec<u8>,
    overflow: bool,
    management_busy: bool,
    #[cfg(test)]
    payload_writes: Vec<usize>,
    staging: Option<String>,
    kind: UploadKind,
    upload: Option<Upload>,
    completed_photo: Option<String>,
}

impl Session {
    pub fn set_ota_active(&mut self, active: bool) {
        self.ota_active = active;
    }

    pub fn set_management_busy(&mut self, busy: bool) {
        self.management_busy = busy;
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// The host went away: drop a half-written file and the staging area.
    pub fn reset(&mut self, vfs: &mut Vfs) {
        self.ota_body = None;
        self.ota_active = false;
        self.ota_poisoned = false;
        self.completed_photo = None;
        if let Some(up) = self.upload.take() {
            let _ = vfs.close(up.fd);
        }
        if self.staging.take().is_some() {
            let _ = vfs.remove_tree(self.staging_root());
        }
        self.line.clear();
        self.overflow = false;
    }
}
