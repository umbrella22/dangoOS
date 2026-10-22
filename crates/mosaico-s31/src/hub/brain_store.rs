//! Durable resident-brain source. A candidate is validated by the UI-owned VM
//! before `persist` runs; the atomic file rename precedes the runtime swap.
use alloc::{format, string::String};
use mosaico_fs::{Errno, Kind, OpenFlags, Vfs};

use crate::behavior::brain::SOURCE_MAX;

pub const ROOT: &str = "/data/brain";
pub const ACTIVE: &str = "/data/brain/active.lua";
pub const STAGING: &str = "/data/brain/.incoming";
pub const STAGED_SOURCE: &str = "/data/brain/.incoming/resident/source.lua";
const NEXT: &str = "/data/brain/.next.lua";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Install(String),
    Reload,
    Default,
    Status,
}

impl Action {
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Install(_) => "brain-commit",
            Self::Reload => "brain-reload",
            Self::Default => "brain-default",
            Self::Status => "brain-status",
        }
    }
}

pub fn read_source(vfs: &mut Vfs, path: &str) -> Result<String, String> {
    let meta = vfs.stat(path).map_err(|e| format!("brain source: {e}"))?;
    if meta.kind != Kind::File || meta.size == 0 || meta.size as usize > SOURCE_MAX {
        return Err("brain source must be a nonempty file of at most 64 KiB".into());
    }
    let bytes = vfs
        .read_file(path)
        .map_err(|e| format!("brain read: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "brain source must be UTF-8".into())
}

/// Only an absent active file means the bundled default was selected. Damaged
/// or unreadable files remain errors, so boot records the native fallback.
pub fn selected(vfs: &mut Vfs) -> Result<Option<String>, String> {
    if !vfs.mount_names().any(|name| name == "data") {
        return Err("brain data filesystem unavailable".into());
    }
    match vfs.stat(ACTIVE) {
        Err(Errno::NOENT) => Ok(None),
        Err(e) => Err(format!("brain selection: {e}")),
        Ok(_) => read_source(vfs, ACTIVE).map(Some),
    }
}

/// Call only after initializing and exercising the isolated candidate VM.
/// A failed write/close/rename preserves the previous active file. An
/// interrupted temporary write is ignored on boot and overwritten next time.
pub fn persist(vfs: &mut Vfs, source: Option<&str>) -> Result<(), String> {
    if !vfs.mount_names().any(|name| name == "data") {
        return Err("brain data filesystem unavailable".into());
    }
    let Some(source) = source else {
        return match vfs.unlink(ACTIVE) {
            Ok(()) | Err(Errno::NOENT) => Ok(()),
            Err(e) => Err(format!("brain default: {e}")),
        };
    };
    if source.is_empty() || source.len() > SOURCE_MAX {
        return Err("brain source must contain 1..65536 bytes".into());
    }
    vfs.create_dir_all(ROOT)
        .map_err(|e| format!("brain root: {e}"))?;
    let fd = vfs
        .open(NEXT, OpenFlags::WRITE)
        .map_err(|e| format!("brain open: {e}"))?;
    let written = (|| {
        if vfs.write(fd, source.as_bytes())? != source.len() {
            return Err(Errno::NOSPC);
        }
        vfs.fsync(fd)
    })();
    let closed = vfs.close(fd);
    written
        .and(closed)
        .map_err(|e| format!("brain write: {e}"))?;
    vfs.rename(NEXT, ACTIVE)
        .map_err(|e| format!("brain rename: {e}"))
}

/// The console protocol has one bounded reply line; scripts may throw arbitrary
/// text. Preserve a UTF-8 prefix while preventing reply/log-line injection.
pub fn error_line(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c == '\r' || c == '\n' {
            break;
        }
        let c = if c.is_control() { ' ' } else { c };
        if out.len() + c.len_utf8() > 96 {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use mosaico_fs::{Fs, Geometry, RamDevice};

    fn vfs() -> Vfs {
        let mut v = Vfs::new();
        let (fs, _) =
            Fs::mount_or_format(Box::new(RamDevice::new(4096, 16)), Geometry::flash(16)).unwrap();
        v.mount("data", fs);
        v
    }

    #[test]
    fn failed_candidate_and_full_storage_preserve_runtime_and_persisted_source() {
        use crate::behavior::brain::Controller;
        let old = "return {on_event=function()end,tick=function()fox.set_expression('Happy')end}";
        let new = "return {on_event=function()end,tick=function()fox.set_expression('Angry')end}";
        let mut v = vfs();
        persist(&mut v, Some(old)).unwrap();
        let mut brain = Controller::new(old, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        assert!(brain
            .replace_with_commit("error('bad candidate')", 1, || persist(&mut v, Some("bad")))
            .is_err());
        assert_eq!(selected(&mut v).unwrap().as_deref(), Some(old));
        assert_eq!(brain.source(), old);
        let huge = alloc::format!("{new}\n--{}", "x".repeat(60_000));
        let mut attempted = false;
        assert!(
            brain
                .replace_with_commit(&huge, 2, || {
                    attempted = true;
                    persist(&mut v, Some(&huge))
                })
                .is_err(),
            "16-sector FS cannot fit a second 60 KiB source"
        );
        assert!(
            attempted,
            "candidate validation must reach the failed durable commit"
        );
        assert_eq!(brain.source(), old);
        assert_eq!(selected(&mut v).unwrap().as_deref(), Some(old));
        assert_eq!(v.open_files(), 0);
        v.remove_tree(NEXT).unwrap();
        brain
            .replace_with_commit(new, 2, || persist(&mut v, Some(new)))
            .unwrap();
        assert_eq!(brain.source(), new);
        assert_eq!(selected(&mut v).unwrap().as_deref(), Some(new));
        assert_eq!(brain.output().unwrap().expression, crate::Expression::Angry);
    }

    #[test]
    fn temporary_writes_are_ignored_and_only_absence_selects_default() {
        let mut v = vfs();
        assert_eq!(selected(&mut v).unwrap(), None);
        persist(&mut v, Some("return {}")).unwrap();
        v.write_file(NEXT, b"broken").unwrap();
        assert_eq!(selected(&mut v).unwrap().as_deref(), Some("return {}"));
        persist(&mut v, None).unwrap();
        assert_eq!(selected(&mut v).unwrap(), None);
        v.write_file(ACTIVE, &[0xff]).unwrap();
        assert!(selected(&mut v).is_err());
        // A missing mount is a real failure, never a default selection.
        assert!(selected(&mut Vfs::new()).is_err());
        assert!(persist(&mut Vfs::new(), None).is_err());
        assert!(!error_line("中文\n@ok injected").contains('\n'));
        assert_eq!(error_line("中文\n@ok injected"), "中文");
        assert!(error_line(&"狐".repeat(100)).len() <= 96);
    }
}
