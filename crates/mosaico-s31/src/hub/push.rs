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
//! ```
//!
//! `begin` stages a fresh `/data/apps/.incoming/<id>`; `file` writes into it
//! (the CRC-32 is the zlib one, in hex); `commit` checks the manifest, swaps
//! the staged directory for `/data/apps/<id>` and asks for a rescan. Apps in
//! `/system` cannot be changed, but a `/data` app with the same id shadows
//! one, and `rm` brings it back.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use mosaico_fs::{Errno, Kind, OpenFlags, Vfs};

use super::registry::{self, id_is_valid, MANIFEST, MANIFEST_MAX_BYTES};

pub const PREFIX: &str = "@mosaico ";
pub const VERSION: u32 = 1;
const DATA_APPS: &str = "/data/apps";
const STAGING: &str = "/data/apps/.incoming";
const PHOTO_STAGING: &str = "/data/appdata/album_app/.incoming";
const PHOTO_ROOT: &str = "/data/appdata/album_app/photos";
/// Longest command line; longer ones are discarded whole.
const LINE_MAX: usize = 256;
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
    Manage(super::uninstall::Operation),
    Stop,
    /// Reset the chip once the reply is out.
    Reboot,
    /// Reload the persisted skin selection on the UI task.
    SkinsChanged,
    /// The UI task validates/commits the resident brain and returns the reply.
    Brain(super::brain_store::Action),
    Ota(crate::ota::Command),
    /// Explicit one-shot diagnostic reset control; unavailable in normal builds.
    #[cfg(feature = "usb-session-diagnostics")]
    UsbDiagReset(bool),
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

    /// Feed bytes from the host; effects come out in order.
    pub fn feed(&mut self, vfs: &mut Vfs, mut data: &[u8], out: &mut dyn FnMut(Effect)) {
        while !data.is_empty() {
            let consumed = self.feed_until_deferred(vfs, data, out);
            data = &data[consumed..];
        }
    }

    /// Consume whole payload chunks, but stop exactly after a deferred
    /// management command. Execute it before feeding the unconsumed tail.
    pub fn feed_until_deferred(
        &mut self,
        vfs: &mut Vfs,
        mut data: &[u8],
        out: &mut dyn FnMut(Effect),
    ) -> usize {
        let original_len = data.len();
        while !data.is_empty() {
            if self.ota_poisoned {
                return original_len;
            }
            if let Some(body) = self.ota_body.as_mut() {
                let n = (body.len - body.bytes.len()).min(data.len());
                body.bytes.extend_from_slice(&data[..n]);
                data = &data[n..];
                if body.bytes.len() == body.len {
                    let body = self.ota_body.take().unwrap();
                    if body.invalid {
                        out(Effect::Reply(
                            "@err ota-chunk invalid header; body discarded".into(),
                        ));
                    } else {
                        out(Effect::Ota(crate::ota::Command::Chunk {
                            offset: body.offset,
                            bytes: body.bytes,
                            crc: body.crc,
                        }));
                    }
                    return original_len - data.len();
                }
                continue;
            }
            if self.upload.is_some() {
                data = self.feed_upload(vfs, data, out);
                continue;
            }
            match data.iter().position(|&b| b == b'\n') {
                Some(n) => {
                    self.push_line(&data[..n]);
                    data = &data[n + 1..];
                    let line = core::mem::take(&mut self.line);
                    let overflow = core::mem::replace(&mut self.overflow, false);
                    if overflow || core::str::from_utf8(&line).is_err() {
                        // The discarded prefix may have been a binary-body header.
                        // Its length is unknowable; only reconnect can restore framing.
                        self.ota_active = false;
                        self.ota_poisoned = true;
                        out(Effect::OtaCancel);
                        out(Effect::Reply(if overflow {
                            "@err line too long; reconnect required".into()
                        } else {
                            "@err invalid UTF-8 control line; reconnect required".into()
                        }));
                        return original_len - data.len();
                    } else if let Ok(text) = core::str::from_utf8(&line) {
                        let text = text.trim_end_matches('\r');
                        if let Some(cmd) = text.strip_prefix(PREFIX) {
                            let mut deferred = false;
                            self.command(vfs, cmd.trim(), &mut |effect| {
                                deferred |= matches!(
                                    effect,
                                    Effect::Manage(_)
                                        | Effect::Brain(_)
                                        | Effect::Ota(_)
                                        | Effect::OtaCancel
                                );
                                out(effect);
                            });
                            if deferred {
                                return original_len - data.len();
                            }
                            if self.upload.as_ref().is_some_and(|u| u.left == 0) {
                                // An empty file: no payload follows.
                                self.feed_upload(vfs, &[], out);
                            }
                        }
                    }
                }
                None => {
                    self.push_line(data);
                    data = &[];
                }
            }
        }
        original_len
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

    fn push_line(&mut self, bytes: &[u8]) {
        if self.line.len() + bytes.len() > LINE_MAX {
            self.overflow = true;
        } else if !self.overflow {
            self.line.extend_from_slice(bytes);
        }
    }

    fn feed_upload<'d>(
        &mut self,
        vfs: &mut Vfs,
        data: &'d [u8],
        out: &mut dyn FnMut(Effect),
    ) -> &'d [u8] {
        let up = self.upload.as_mut().expect("upload");
        let n = (up.left as usize).min(data.len());
        let (chunk, rest) = data.split_at(n);
        if up.failed.is_none() && vfs.stat(&up.path).is_err() {
            up.failed = Some(Errno::NOENT);
        }
        if up.failed.is_none() && n > 0 {
            #[cfg(test)]
            self.payload_writes.push(n);
            if let Err(e) =
                vfs.write(up.fd, chunk)
                    .and_then(|w| if w == n { Ok(()) } else { Err(Errno::NOSPC) })
            {
                up.failed = Some(e);
            }
        }
        up.crc = crc32_update(up.crc, chunk);
        up.left -= n as u32;
        if up.left == 0 {
            let up = self.upload.take().expect("upload");
            let closed = vfs.close(up.fd);
            let crc = !up.crc;
            if self.kind == UploadKind::Photo {
                self.completed_photo = (up.failed.is_none() && closed.is_ok() && crc == up.want)
                    .then(|| up.rel.clone());
            }
            let reply = match (up.failed, closed) {
                (Some(e), _) | (None, Err(e)) => format!("@err file {}: {e}", up.rel),
                _ if crc != up.want => {
                    format!("@err file {}: crc {crc:08x} != {:08x}", up.rel, up.want)
                }
                _ => format!("@ok file {}", up.rel),
            };
            if reply.starts_with("@err") {
                let _ = vfs.unlink(&up.path);
            }
            out(Effect::Reply(reply));
        }
        rest
    }

    fn ota_command(&mut self, verb: &str, args: &[&str], out: &mut dyn FnMut(Effect)) {
        use crate::ota::Command;
        let result: core::result::Result<Option<Command>, &str> = match (verb, args) {
            ("ota-status", []) => Ok(Some(Command::Status)),
            ("ota-abort", []) => {
                self.ota_active = false;
                Ok(Some(Command::Abort))
            }
            ("ota-end", []) => Ok(Some(Command::End)),
            ("ota-begin", [size, sha]) => {
                if self.management_busy || self.staging.is_some() || self.upload.is_some() {
                    Err("other upload/management busy")
                } else {
                    match (size.parse::<u32>(), crate::ota::parse_sha(sha)) {
                        (Ok(size), Ok(sha)) => Ok(Some(Command::Begin { size, sha })),
                        _ => Err("invalid size/SHA256"),
                    }
                }
            }
            ("ota-chunk", _) => {
                let len = args.get(1).and_then(|s| s.parse::<usize>().ok());
                match len {
                    Some(len) if (1..=crate::ota::SECTOR).contains(&len) => {
                        let offset = args.first().and_then(|s| s.parse::<u32>().ok());
                        let crc = args
                            .get(2)
                            .filter(|s| s.len() == 8)
                            .and_then(|s| u32::from_str_radix(s, 16).ok());
                        self.ota_body = Some(OtaBody {
                            offset: offset.unwrap_or(0),
                            len,
                            crc: crc.unwrap_or(0),
                            bytes: Vec::with_capacity(len),
                            invalid: args.len() != 3 || offset.is_none() || crc.is_none(),
                        });
                        if args.len() != 3 || offset.is_none() || crc.is_none() {
                            self.ota_active = false;
                            out(Effect::OtaCancel);
                        }
                        Ok(None)
                    }
                    _ => {
                        self.ota_active = false;
                        self.ota_poisoned = true;
                        out(Effect::OtaCancel);
                        Err("invalid body length; reconnect required")
                    }
                }
            }
            _ => Err("invalid OTA command"),
        };
        match result {
            Ok(Some(cmd)) => out(Effect::Ota(cmd)),
            Ok(None) => {}
            Err(e) => out(Effect::Reply(format!("@err {verb}: {e}"))),
        }
    }

    fn command(&mut self, vfs: &mut Vfs, cmd: &str, out: &mut dyn FnMut(Effect)) {
        let mut words = cmd.split_ascii_whitespace();
        let verb = words.next().unwrap_or("");
        let args: Vec<&str> = words.collect();
        if verb.starts_with("ota-") {
            self.ota_command(verb, &args, out);
            return;
        }
        if self.ota_active && !matches!(verb, "hello") {
            out(Effect::Reply("@err OTA busy".into()));
            return;
        }
        if self.management_busy
            && matches!(
                verb,
                "run"
                    | "begin"
                    | "file"
                    | "commit"
                    | "rm"
                    | "rmdata"
                    | "photo-begin"
                    | "photo-commit"
            )
        {
            out(Effect::Reply("@err app management busy".into()));
            return;
        }
        let result = match (verb, args.as_slice()) {
            ("hello", []) => Ok(format!("hello mosaico-push {VERSION}")),
            ("begin", [id]) => self.begin(vfs, id, UploadKind::App),
            ("skin-begin", [id]) => self.begin(vfs, id, UploadKind::Skin),
            ("photo-begin", []) => self
                .begin(vfs, "photo", UploadKind::Photo)
                .map(|_| "photo-begin".into()),
            ("photo-commit", [rel]) => self.commit_photo(vfs, rel),
            ("brain-begin", []) => self
                .begin(vfs, "resident", UploadKind::Brain)
                .map(|_| "brain-begin".into()),
            ("brain-commit", [])
                if self.kind == UploadKind::Brain
                    && self.staging.as_deref() == Some("resident") =>
            {
                match super::brain_store::read_source(vfs, super::brain_store::STAGED_SOURCE) {
                    Ok(source) => {
                        self.staging = None;
                        let _ = vfs.remove_tree(self.staging_root());
                        out(Effect::Brain(super::brain_store::Action::Install(source)));
                        return;
                    }
                    Err(e) => Err(e),
                }
            }
            (verb @ ("brain-reload" | "brain-default" | "brain-status"), []) => {
                use super::brain_store::Action;
                let action = match verb {
                    "brain-reload" => Action::Reload,
                    "brain-default" => Action::Default,
                    _ => Action::Status,
                };
                out(Effect::Brain(action));
                return;
            }
            ("skin-commit", [id])
                if self.kind == UploadKind::Skin && self.staging.as_deref() == Some(id) =>
            {
                match super::skins::commit(vfs, id) {
                    Ok(()) => {
                        self.staging = None;
                        out(Effect::SkinsChanged);
                        Ok(format!("skin-commit {id}"))
                    }
                    Err(e) => Err(e),
                }
            }
            ("skin-select", [id]) => super::skins::select(vfs, id).map(|()| {
                out(Effect::SkinsChanged);
                format!("skin-select {id}")
            }),
            ("skin-rm", [id]) => {
                let result = super::skins::remove(vfs, id);
                out(Effect::SkinsChanged);
                result.map(|()| format!("skin-rm {id}"))
            }
            #[cfg(feature = "usb-session-diagnostics")]
            (verb @ ("usb-diag-reset-arm" | "usb-diag-reset-cancel"), []) => {
                out(Effect::UsbDiagReset(verb == "usb-diag-reset-arm"));
                return;
            }
            ("skin-ls", []) => super::skins::list(vfs),
            ("file", [rel, size, crc]) => match self.file(vfs, rel, size, crc) {
                // The host sends the payload only after this; the final
                // reply follows it (see `feed_upload`).
                Ok(()) => Ok(format!("ready {rel}")),
                Err(text) => Err(text),
            },
            ("commit", [id]) => self.commit(vfs, id, false, out),
            ("commit", [id, "run"]) => self.commit(vfs, id, true, out),
            ("abort", []) => {
                self.completed_photo = None;
                if self.staging.take().is_some() {
                    let _ = vfs.remove_tree(self.staging_root());
                }
                Ok("abort".into())
            }
            ("run", [id]) if id_is_valid(id) => {
                out(Effect::Run(id.to_string()));
                Ok(format!("run {id}"))
            }
            ("stop", []) => {
                out(Effect::Stop);
                Ok("stop".into())
            }
            ("reload", []) => {
                out(Effect::Reload { run: None });
                Ok("reload".into())
            }
            (verb @ ("rm" | "rmdata"), [id]) if id_is_valid(id) => {
                match super::uninstall::resolve(vfs, id) {
                    Ok(identity) => {
                        // Invalidate in-memory staging as well as the on-disk
                        // incoming tree, preventing a stale file/commit resurrection.
                        if (self.kind == UploadKind::App && self.staging.as_deref() == Some(id))
                            || (self.kind == UploadKind::Photo && *id == "album_app")
                        {
                            self.staging = None;
                            self.completed_photo = None;
                        }
                        let op = if verb == "rm" {
                            super::uninstall::Operation::Remove(identity)
                        } else {
                            super::uninstall::Operation::Clear(identity)
                        };
                        out(Effect::Manage(op));
                        return; // completion reply belongs to the executor
                    }
                    Err(e) => Err(e),
                }
            }
            ("ls", []) => list(vfs),
            ("status", []) => Ok(status(vfs)),
            ("reboot", []) => {
                out(Effect::Reboot);
                Ok("reboot".into())
            }
            _ => Err(format!("bad command: {cmd}")),
        };
        out(Effect::Reply(match result {
            Ok(text) => format!("@ok {text}"),
            Err(text) => format!("@err {text}"),
        }));
    }

    fn staging_root(&self) -> &'static str {
        match self.kind {
            UploadKind::App => STAGING,
            UploadKind::Skin => super::skins::STAGING,
            UploadKind::Brain => super::brain_store::STAGING,
            UploadKind::Photo => PHOTO_STAGING,
        }
    }

    fn begin(&mut self, vfs: &mut Vfs, id: &str, kind: UploadKind) -> Result<String, String> {
        self.completed_photo = None;
        if !id_is_valid(id) {
            return Err(format!("bad id {id}"));
        }
        if kind == UploadKind::App {
            super::uninstall::ensure_ready(vfs, id)?;
        }
        if kind == UploadKind::Photo {
            super::uninstall::ensure_ready(vfs, "album_app")?;
        }
        if kind == UploadKind::Skin && id == "default" {
            return Err("reserved skin id default".into());
        }
        if self.staging.is_some() {
            let _ = vfs.remove_tree(self.staging_root());
        }
        self.kind = kind;
        let dir = format!("{}/{id}", self.staging_root());
        match vfs.remove_tree(self.staging_root()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(e) => return Err(format!("clear staging: {e}")),
        }
        vfs.create_dir_all(&dir)
            .map_err(|e| format!("{dir}: {e}"))?;
        self.staging = Some(id.to_string());
        Ok(format!("begin {id}"))
    }

    fn file(&mut self, vfs: &mut Vfs, rel: &str, size: &str, crc: &str) -> Result<(), String> {
        self.completed_photo = None;
        let id = self.staging.as_deref().ok_or("file before begin")?;
        if vfs.stat(&format!("{}/{id}", self.staging_root())).is_err() {
            return Err("staging invalidated".into());
        }
        let size: u32 = size.parse().map_err(|_| format!("bad size {size}"))?;
        let want = u32::from_str_radix(crc, 16).map_err(|_| format!("bad crc {crc}"))?;
        if self.kind == UploadKind::Photo && (!photo_path_is_safe(rel) || size == 0) {
            return Err("photo upload requires a nonempty JPG/JPEG/PNG/BIN file".into());
        }
        if self.kind == UploadKind::Brain
            && (rel != "source.lua"
                || size == 0
                || size as usize > crate::behavior::brain::SOURCE_MAX)
        {
            return Err("brain upload requires source.lua of 1..65536 bytes".into());
        }
        if !rel_is_safe(rel) {
            return Err(format!("bad path {rel}"));
        }
        if size > FILE_MAX {
            return Err(format!("{rel} is larger than {FILE_MAX} bytes"));
        }
        let path = format!("{}/{id}/{rel}", self.staging_root());
        if let Some((dir, _)) = path.rsplit_once('/') {
            vfs.create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
        }
        let fd = vfs
            .open(&path, OpenFlags::WRITE)
            .map_err(|e| format!("{rel}: {e}"))?;
        self.upload = Some(Upload {
            fd,
            rel: rel.to_string(),
            path,
            left: size,
            crc: !0,
            want,
            failed: None,
        });
        Ok(())
    }

    fn commit_photo(&mut self, vfs: &mut Vfs, rel: &str) -> Result<String, String> {
        if self.kind != UploadKind::Photo || self.staging.as_deref() != Some("photo") {
            return Err("photo-commit: not staged".into());
        }
        if !photo_path_is_safe(rel) {
            return Err("invalid photo path".into());
        }
        // A stale file is insufficient evidence: failed-transfer cleanup can
        // itself fail. Only this session's successfully synced CRC may publish.
        if self.completed_photo.as_deref() != Some(rel) {
            return Err("photo is missing or transfer failed".into());
        }
        super::uninstall::ensure_ready(vfs, "album_app")?;
        let source = format!("{PHOTO_STAGING}/photo/{rel}");
        match vfs.stat(&source) {
            Ok(meta) if meta.kind == Kind::File && meta.size > 0 => {}
            _ => return Err("photo is missing or transfer failed".into()),
        }
        let target = format!("{PHOTO_ROOT}/{rel}");
        let parent = target.rsplit_once('/').expect("photo parent").0;
        vfs.create_dir_all(parent)
            .map_err(|e| format!("photo directory: {e}"))?;
        // Successful file close synced its contents. Same-filesystem rename
        // publishes the complete image atomically and preserves the old image
        // on failure. Clearing album data also removes this staging tree.
        vfs.rename(&source, &target)
            .map_err(|e| format!("photo commit: {e}"))?;
        self.staging = None;
        self.completed_photo = None;
        let _ = vfs.remove_tree(PHOTO_STAGING);
        Ok(format!("photo-commit {rel}"))
    }

    fn commit(
        &mut self,
        vfs: &mut Vfs,
        id: &str,
        run: bool,
        out: &mut dyn FnMut(Effect),
    ) -> Result<String, String> {
        if self.kind != UploadKind::App || self.staging.as_deref() != Some(id) {
            return Err(format!("commit {id}: not staged"));
        }
        super::uninstall::ensure_ready(vfs, id)?;
        let staged = format!("{STAGING}/{id}");
        let text = registry::read_text(vfs, &format!("{staged}/{MANIFEST}"), MANIFEST_MAX_BYTES)?;
        let m = registry::parse_manifest(&text, id)?;
        match vfs.stat(&format!("{staged}/{}", m.entry)) {
            Ok(meta) if meta.kind == Kind::File => {}
            _ => return Err(format!("entry {} missing", m.entry)),
        }
        let target = format!("{DATA_APPS}/{id}");
        match vfs.remove_tree(&target) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(e) => return Err(format!("{target}: {e}")),
        }
        vfs.rename(&staged, &target)
            .map_err(|e| format!("rename: {e}"))?;
        let _ = vfs.remove_tree(self.staging_root());
        self.staging = None;
        out(Effect::Reload {
            run: run.then(|| id.to_string()),
        });
        Ok(format!("commit {id}"))
    }
}

/// A file path inside an app: plain components of `[A-Za-z0-9._-]`, none
/// starting with a dot.
pub fn rel_is_safe(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    registry::relative_path_is_valid(rel)
        && parts.len() <= DEPTH_MAX
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 48
                && !p.starts_with('.')
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        })
}

fn photo_path_is_safe(rel: &str) -> bool {
    rel_is_safe(rel)
        && rel.rsplit_once('.').is_some_and(|(_, ext)| {
            ["jpg", "jpeg", "png", "bin"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

fn status(vfs: &mut Vfs) -> String {
    let names: Vec<String> = vfs.mount_names().map(String::from).collect();
    let mut text = String::from("status");
    if names.is_empty() {
        text.push_str(" no-mounts");
    }
    for name in names {
        match vfs.space(&format!("/{name}")) {
            Ok(sp) => text.push_str(&format!(
                " {name}:{}/{}x{}",
                sp.free, sp.blocks, sp.block_size
            )),
            Err(e) => text.push_str(&format!(" {name}:{e}")),
        }
    }
    text
}

fn list(vfs: &mut Vfs) -> Result<String, String> {
    let mut ids: Vec<String> = match vfs.read_dir(DATA_APPS) {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.meta.kind == Kind::Dir && !e.name.starts_with('.'))
            .map(|e| e.name)
            .collect(),
        Err(Errno::NOENT) => Vec::new(),
        Err(e) => return Err(format!("ls: {e}")),
    };
    ids.sort();
    let mut text = String::from("ls");
    for id in ids {
        text.push(' ');
        text.push_str(&id);
    }
    Ok(text)
}

/// zlib CRC-32 without the final inversion; start from `!0`.
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    crc
}

pub fn crc32(data: &[u8]) -> u32 {
    !crc32_update(!0, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::registry::{IconCache, Registry};
    use mosaico_fs::{Fs, Geometry, RamDevice};

    fn vfs() -> Vfs {
        let mut v = Vfs::new();
        for (name, blocks) in [("system", 32), ("data", 64)] {
            let fs = Fs::mount_or_format(
                Box::new(RamDevice::new(4096, blocks)),
                Geometry::flash(blocks),
            )
            .unwrap()
            .0;
            v.mount(name, fs);
        }
        v
    }

    fn file_cmd(rel: &str, data: &[u8]) -> Vec<u8> {
        let mut out =
            format!("@mosaico file {rel} {} {:08x}\n", data.len(), crc32(data)).into_bytes();
        out.extend_from_slice(data);
        out
    }

    const MANIFEST_TEXT: &str =
        r#"{"schema_version":1,"id":"hello","entry":"scripts/main.lua","display_name":"Hi"}"#;
    const MAIN: &[u8] = b"local x = 1\nreturn x\n";

    fn push_stream(run: bool) -> Vec<u8> {
        let mut s = b"noise from a terminal\n@mosaico hello\r\n@mosaico begin hello\n".to_vec();
        s.extend(file_cmd("launcher.json", MANIFEST_TEXT.as_bytes()));
        s.extend(file_cmd("scripts/main.lua", MAIN));
        s.extend(file_cmd("empty.txt", b""));
        s.extend_from_slice(if run {
            b"@mosaico commit hello run\n"
        } else {
            b"@mosaico commit hello\n"
        });
        s
    }

    fn run(v: &mut Vfs, sess: &mut Session, data: &[u8], chunk: usize) -> Vec<Effect> {
        let mut effects = Vec::new();
        for c in data.chunks(chunk) {
            sess.feed(v, c, &mut |e| effects.push(e));
        }
        effects
    }

    fn replies(effects: &[Effect]) -> Vec<String> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Reply(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn photo_delivery_atomically_replaces_only_the_selected_album_file() {
        for chunk in [1, 7, 64, 1024] {
            let mut v = vfs();
            v.create_dir_all(PHOTO_ROOT).unwrap();
            v.write_file(&format!("{PHOTO_ROOT}/first.jpg"), b"old")
                .unwrap();
            v.write_file(&format!("{PHOTO_ROOT}/keep.bin"), b"keep")
                .unwrap();
            v.write_file("/data/legacy.jpg", b"legacy").unwrap();
            let mut s = Session::new();
            let mut input = b"@mosaico photo-begin\n".to_vec();
            input.extend(file_cmd("first.jpg", b"replacement\0pixels"));
            let r = replies(&run(&mut v, &mut s, &input, chunk));
            assert_eq!(r.last().unwrap(), "@ok file first.jpg");
            assert_eq!(
                v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
                b"old"
            );
            let r = replies(&run(
                &mut v,
                &mut s,
                b"@mosaico photo-commit first.jpg\n",
                chunk,
            ));
            assert_eq!(r, ["@ok photo-commit first.jpg"]);
            assert_eq!(
                v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
                b"replacement\0pixels"
            );
            assert_eq!(
                v.read_file(&format!("{PHOTO_ROOT}/keep.bin")).unwrap(),
                b"keep"
            );
            assert_eq!(v.read_file("/data/legacy.jpg").unwrap(), b"legacy");
            assert!(v.stat(PHOTO_STAGING).is_err());
            let r = replies(&run(
                &mut v,
                &mut s,
                b"@mosaico photo-commit first.jpg\n",
                chunk,
            ));
            assert!(r[0].contains("not staged"));
        }
    }

    #[test]
    fn failed_photo_crc_disconnect_and_album_clear_cannot_publish_partial_data() {
        let mut v = vfs();
        v.create_dir_all(PHOTO_ROOT).unwrap();
        v.write_file(&format!("{PHOTO_ROOT}/first.jpg"), b"old")
            .unwrap();
        let mut s = Session::new();
        let r = replies(&run(&mut v, &mut s, b"@mosaico photo-begin\n@mosaico file first.jpg 3 00000000\nnew@mosaico photo-commit first.jpg\n", 7));
        assert!(r.iter().any(|r| r.contains("crc")));
        assert!(r.last().unwrap().contains("transfer failed"));
        // Model a failed unlink leaving bytes behind after a CRC error. A
        // later commit must still require a successful transfer receipt.
        v.write_file(&format!("{PHOTO_STAGING}/photo/first.jpg"), b"bad")
            .unwrap();
        let r = replies(&run(
            &mut v,
            &mut s,
            b"@mosaico photo-commit first.jpg\n",
            64,
        ));
        assert!(r[0].contains("transfer failed"));
        assert_eq!(
            v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
            b"old"
        );
        run(
            &mut v,
            &mut s,
            b"@mosaico photo-begin\n@mosaico file first.jpg 20 00000000\npartial",
            7,
        );
        s.reset(&mut v);
        assert!(v.stat(PHOTO_STAGING).is_err());
        assert_eq!(
            v.read_file(&format!("{PHOTO_ROOT}/first.jpg")).unwrap(),
            b"old"
        );
        let mut input = b"@mosaico photo-begin\n".to_vec();
        input.extend(file_cmd("dated/new.JPEG", b"complete"));
        run(&mut v, &mut s, &input, 64);
        // N7 clear-data removes appdata, including the live transfer's staging.
        v.remove_tree("/data/appdata/album_app").unwrap();
        let r = replies(&run(
            &mut v,
            &mut s,
            b"@mosaico photo-commit dated/new.JPEG\n",
            64,
        ));
        assert!(r[0].contains("transfer failed"));
        assert!(v.stat("/data/appdata/album_app").is_err());
    }

    #[test]
    fn photo_paths_kind_and_management_barrier_are_enforced() {
        let mut v = vfs();
        let mut s = Session::new();
        run(&mut v, &mut s, b"@mosaico photo-begin\n", 64);
        for path in ["../escape.jpg", "/data/other.jpg", "x.lua", "hidden/.x.jpg"] {
            let r = replies(&run(
                &mut v,
                &mut s,
                format!("@mosaico file {path} 1 00000000\n").as_bytes(),
                64,
            ));
            assert!(r[0].starts_with("@err"), "{r:?}");
        }
        let r = replies(&run(
            &mut v,
            &mut s,
            b"@mosaico commit photo\n@mosaico brain-commit\n",
            64,
        ));
        assert!(r.iter().all(|r| r.starts_with("@err")));
        s.set_management_busy(true);
        let r = replies(&run(
            &mut v,
            &mut s,
            b"@mosaico photo-begin\n@mosaico photo-commit x.jpg\n",
            64,
        ));
        assert_eq!(r, ["@err app management busy", "@err app management busy"]);
        s.set_management_busy(false);
        v.create_dir_all("/data/apps/.clear/album_app").unwrap();
        let r = replies(&run(&mut v, &mut s, b"@mosaico photo-begin\n", 64));
        assert!(r[0].contains("pending recovery"));
    }

    #[test]
    fn brain_transfer_is_scoped_and_commit_is_deferred_before_next_command() {
        use super::super::brain_store::{self, Action};
        for chunk in [1, 64, 512] {
            let mut v = vfs();
            brain_store::persist(&mut v, Some("old source")).unwrap();
            let mut session = Session::new();
            let mut stream = b"@mosaico brain-begin\n".to_vec();
            stream.extend(file_cmd("source.lua", MAIN));
            let replies = replies(&run(&mut v, &mut session, &stream, chunk));
            assert_eq!(
                replies,
                [
                    "@ok brain-begin",
                    "@ok ready source.lua",
                    "@ok file source.lua"
                ]
            );
            let packet = b"@mosaico brain-commit\n@mosaico brain-status\n";
            let mut effects = Vec::new();
            let used = session.feed_until_deferred(&mut v, packet, &mut |e| effects.push(e));
            assert_eq!(&packet[used..], b"@mosaico brain-status\n");
            assert_eq!(
                effects,
                [Effect::Brain(Action::Install(
                    String::from_utf8(MAIN.to_vec()).unwrap()
                ))]
            );
            assert_eq!(
                brain_store::selected(&mut v).unwrap().as_deref(),
                Some("old source"),
                "transport never activates a VM or overwrites active source"
            );
            assert!(v.stat(brain_store::STAGED_SOURCE).is_err());
            assert!(run(&mut v, &mut session, b"@mosaico brain-commit\n", chunk)
                .iter()
                .all(|e| matches!(e, Effect::Reply(s) if s.starts_with("@err"))));
        }
    }

    #[test]
    fn bad_or_interrupted_brain_upload_cannot_touch_active_source() {
        use super::super::brain_store;
        let mut v = vfs();
        brain_store::persist(&mut v, Some("old source")).unwrap();
        let mut session = Session::new();
        let effects = run(&mut v, &mut session, b"@mosaico brain-begin\n@mosaico file other.lua 1 0\n@mosaico file source.lua 0 0\n@mosaico file source.lua 65537 0\n@mosaico file source.lua 3 0\nabc@mosaico brain-commit\n", 64);
        let text = replies(&effects);
        assert!(text[1..4].iter().all(|e| e.starts_with("@err")));
        assert!(text[5].contains("crc"));
        assert!(text[6].starts_with("@err"));
        run(
            &mut v,
            &mut session,
            b"@mosaico file source.lua 30 0\npartial",
            64,
        );
        assert_eq!(v.open_files(), 1);
        session.reset(&mut v);
        assert_eq!(v.open_files(), 0);
        assert_eq!(
            brain_store::selected(&mut v).unwrap().as_deref(),
            Some("old source")
        );
        assert!(v.stat(brain_store::STAGING).is_err());
        // A later app upload has its own staging root and cannot be committed
        // through the brain command, even when its app id is "resident".
        run(&mut v, &mut session, b"@mosaico begin resident\n", 64);
        assert!(
            replies(&run(&mut v, &mut session, b"@mosaico brain-commit\n", 64))[0]
                .starts_with("@err")
        );
    }

    #[test]
    fn push_commits_app_at_any_chunking() {
        for chunk in [1, 7, 64, 512, 1 << 20] {
            let mut v = vfs();
            let mut sess = Session::new();
            let effects = run(&mut v, &mut sess, &push_stream(true), chunk);
            assert_eq!(
                replies(&effects),
                [
                    "@ok hello mosaico-push 1",
                    "@ok begin hello",
                    "@ok ready launcher.json",
                    "@ok file launcher.json",
                    "@ok ready scripts/main.lua",
                    "@ok file scripts/main.lua",
                    "@ok ready empty.txt",
                    "@ok file empty.txt",
                    "@ok commit hello"
                ],
                "chunk {chunk}"
            );
            assert!(effects.contains(&Effect::Reload {
                run: Some("hello".into())
            }));
            assert_eq!(
                v.read_file("/data/apps/hello/scripts/main.lua").unwrap(),
                MAIN
            );
            assert_eq!(v.read_file("/data/apps/hello/empty.txt").unwrap(), b"");
            assert!(v.stat(STAGING).is_err(), "staging is cleaned up");
            let reg = Registry::scan(&mut v, &mut IconCache::new(), &mut |_| {});
            let i = reg.find("hello").expect("listed");
            assert!(reg.apps[i].runnable());
            assert_eq!(reg.apps[i].title, "Hi");
        }
    }

    #[test]
    fn commit_replaces_old_files_and_rm_removes() {
        let mut v = vfs();
        v.create_dir_all("/data/apps/hello/old").unwrap();
        v.write_file("/data/apps/hello/old/stale.lua", b"x")
            .unwrap();
        let mut sess = Session::new();
        run(&mut v, &mut sess, &push_stream(false), 512);
        assert!(v.stat("/data/apps/hello/old/stale.lua").is_err());
        let e = run(&mut v, &mut sess, b"@mosaico rm hello\n", 512);
        assert!(
            replies(&e).is_empty(),
            "queueing must never claim completion"
        );
        assert!(
            v.stat("/data/apps/hello").is_ok(),
            "feed cannot delete a running app"
        );
        let Effect::Manage(op) = e[0] else {
            panic!("expected deferred operation")
        };
        super::super::uninstall::execute(&mut v, op).unwrap();
        assert!(v.stat("/data/apps/hello").is_err());
        let r = replies(&run(
            &mut v,
            &mut sess,
            b"@mosaico ls\n@mosaico rm hello\n",
            512,
        ));
        assert_eq!(r[0], "@ok ls");
        assert!(r[1].starts_with("@err app hello not found"));
    }

    #[test]
    fn partial_upload_ui_removal_cannot_resurrect_app_or_commit() {
        let mut v = vfs();
        let mut session = Session::new();
        run(&mut v, &mut session, &push_stream(false), 512);
        run(
            &mut v,
            &mut session,
            b"@mosaico begin hello\n@mosaico file main.lua 6 0\nabc",
            512,
        );
        assert_eq!(v.open_files(), 1);
        let identity = super::super::uninstall::resolve(&mut v, "hello").unwrap();
        super::super::uninstall::execute(
            &mut v,
            super::super::uninstall::Operation::Remove(identity),
        )
        .unwrap();
        let e = run(
            &mut v,
            &mut session,
            b"def@mosaico commit hello\n@mosaico file resurrect.lua 0 0\n",
            512,
        );
        assert!(
            replies(&e).iter().all(|r| r.starts_with("@err")),
            "{:?}",
            replies(&e)
        );
        assert_eq!(v.open_files(), 0);
        assert!(v.stat("/data/apps/hello").is_err());
        assert!(v.stat("/data/apps/.incoming/hello").is_err());
    }
    #[test]
    fn deferred_boundary_preserves_single_packet_tail_and_payload_batching() {
        for chunk in [64, 512] {
            let mut v = vfs();
            let mut session = Session::new();
            run(&mut v, &mut session, &push_stream(false), 512);
            run(&mut v, &mut session, b"@mosaico begin hello\n", 512);
            let packet = b"@mosaico rm hello\n@mosaico commit hello run\n";
            let mut effects = Vec::new();
            let consumed = session.feed_until_deferred(&mut v, packet, &mut |e| effects.push(e));
            assert_eq!(&packet[consumed..], b"@mosaico commit hello run\n");
            let Effect::Manage(op) = effects[0] else {
                panic!("expected deferred delete")
            };
            super::super::uninstall::execute(&mut v, op).unwrap();
            effects.clear();
            session.feed_until_deferred(&mut v, &packet[consumed..], &mut |e| effects.push(e));
            assert!(replies(&effects)[0].starts_with("@err"));
            assert!(v.stat("/data/apps/hello").is_err());
            run(&mut v, &mut session, b"@mosaico begin payload\n", 512);
            let payload = vec![42u8; 4096];
            let header = format!("@mosaico file big.bin 4096 {:08x}\n", crc32(&payload));
            run(&mut v, &mut session, header.as_bytes(), 512);
            session.payload_writes.clear();
            run(&mut v, &mut session, &payload, chunk);
            assert_eq!(
                session.payload_writes,
                vec![chunk; 4096 / chunk],
                "one write per USB block"
            );
        }
    }

    #[test]
    fn management_barrier_rejects_navigation_and_mutations_explicitly() {
        let mut v = vfs();
        let mut session = Session::new();
        session.set_management_busy(true);
        let e = run(&mut v, &mut session, b"@mosaico run snake\n@mosaico begin hello\n@mosaico rmdata snake\n@mosaico commit hello run\n", 512);
        assert_eq!(replies(&e), ["@err app management busy"; 4]);
        assert!(e.iter().all(|e| matches!(e, Effect::Reply(_))));
    }

    #[test]
    fn bad_crc_and_paths_are_rejected() {
        let mut v = vfs();
        let mut sess = Session::new();
        let mut s = b"@mosaico file a.lua 1 0\n@mosaico begin hello\n".to_vec();
        s.extend_from_slice(
            format!("@mosaico file scripts/main.lua {} 12345678\n", MAIN.len()).as_bytes(),
        );
        s.extend_from_slice(MAIN);
        for bad in [
            "../x", ".hidden", "/abs", "a//b", "a/../b", "sp ace", "a\\b",
        ] {
            s.extend_from_slice(format!("@mosaico file {bad} 0 0\n").as_bytes());
        }
        s.extend_from_slice(format!("@mosaico file big.bin {} 0\n", FILE_MAX + 1).as_bytes());
        s.extend_from_slice(b"@mosaico commit other\n@mosaico commit hello\n@mosaico frobnicate\n");
        let r = replies(&run(&mut v, &mut sess, &s, 5));
        assert!(r[0].starts_with("@err file before begin"), "{r:?}");
        assert_eq!(r[1], "@ok begin hello");
        assert_eq!(r[2], "@ok ready scripts/main.lua");
        let r = &r[1..];
        assert!(r[2].contains("crc"), "{r:?}");
        assert!(v
            .stat("/data/apps/.incoming/hello/scripts/main.lua")
            .is_err());
        // "sp ace" splits into four words and is a bad command.
        assert!(r[3..10].iter().all(|t| t.starts_with("@err")), "{r:?}");
        assert!(r[10].contains("larger"), "{r:?}");
        assert!(r[11].contains("not staged"));
        assert!(r[12].contains("launcher.json"), "no manifest: {r:?}");
        assert!(r[13].starts_with("@err bad command"));
        assert_eq!(r.len(), 14);
        assert!(v.stat("/data/apps/hello").is_err());
    }

    #[test]
    fn long_lines_reset_and_controls() {
        let mut v = vfs();
        let mut sess = Session::new();
        let mut s = vec![b'a'; 1000];
        s.extend_from_slice(
            b"\n@mosaico run snake\n@mosaico stop\n@mosaico reload\n@mosaico begin x\n",
        );
        let e = run(&mut v, &mut sess, &s, 100);
        assert_eq!(replies(&e), ["@err line too long; reconnect required"]);
        assert_eq!(e.len(), 2);
        assert!(e.contains(&Effect::OtaCancel));
        assert!(v.stat("/data/apps/.incoming/x").is_err());
        assert!(run(
            &mut v,
            &mut sess,
            b"@mosaico run snake\n@mosaico stop\n",
            100
        )
        .is_empty());
        sess.reset(&mut v);
        let e = run(
            &mut v,
            &mut sess,
            b"@mosaico run snake\n@mosaico stop\n@mosaico reload\n@mosaico begin x\n",
            100,
        );
        let r = replies(&e);
        assert_eq!(
            r,
            ["@ok run snake", "@ok stop", "@ok reload", "@ok begin x"]
        );
        assert!(e.contains(&Effect::Run("snake".into())) && e.contains(&Effect::Stop));
        assert!(v.stat("/data/apps/.incoming/x").is_ok());
        // A half-sent file, then the host disconnects.
        run(&mut v, &mut sess, b"@mosaico file a.lua 10 0\nabc", 100);
        sess.reset(&mut v);
        assert!(v.stat(STAGING).is_err());
        let r = replies(&run(&mut v, &mut sess, b"@mosaico hello\n", 100));
        assert_eq!(r, ["@ok hello mosaico-push 1"]);
    }

    #[test]
    fn status_and_reboot() {
        let mut v = vfs();
        let mut sess = Session::new();
        let e = run(&mut v, &mut sess, b"@mosaico status\n@mosaico reboot\n", 64);
        let r = replies(&e);
        assert!(
            r[0].starts_with("@ok status ") && r[0].contains(" data:62/64x4096"),
            "{r:?}"
        );
        assert!(r[0].contains(" system:"), "{r:?}");
        assert_eq!(r[1], "@ok reboot");
        assert!(e.contains(&Effect::Reboot));
        let mut empty = Vfs::new();
        let r = replies(&run(&mut empty, &mut sess, b"@mosaico status\n", 64));
        assert_eq!(r, ["@ok status no-mounts"]);
    }
    #[test]
    fn diagnostic_reset_commands_are_feature_gated_exact_and_not_binary_controls() {
        let mut v = vfs();
        let mut session = Session::new();
        let effects = run(
            &mut v,
            &mut session,
            b"@mosaico usb-diag-reset-arm\n@mosaico usb-diag-reset-cancel\n",
            3,
        );
        #[cfg(feature = "usb-session-diagnostics")]
        assert_eq!(
            effects,
            [Effect::UsbDiagReset(true), Effect::UsbDiagReset(false)]
        );
        #[cfg(not(feature = "usb-session-diagnostics"))]
        assert!(
            replies(&effects)
                .iter()
                .all(|s| s.starts_with("@err bad command:"))
                && effects.len() == 2
        );
        let effects = run(&mut v, &mut session, b"@mosaico usb-diag-reset-arm 1\n", 64);
        assert!(replies(&effects)[0].starts_with("@err bad command:"));
        run(&mut v, &mut session, b"@mosaico begin hello\n", 64);
        let binary = file_cmd(
            "raw.bin",
            b"@mosaico usb-diag-reset-arm\n@mosaico usb-diag-reset-cancel\n",
        );
        let effects = run(&mut v, &mut session, &binary, 5);
        assert!(effects.iter().all(|e| matches!(e, Effect::Reply(_))));
        assert!(replies(&effects)
            .iter()
            .all(|s| !s.contains("usb-diag-reset")));
    }
}
