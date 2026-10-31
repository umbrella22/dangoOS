//! Staged file transfers, CRC completion and atomic publication boundaries.
use super::{
    Effect, Session, Upload, UploadKind, DATA_APPS, DEPTH_MAX, FILE_MAX, PHOTO_ROOT, PHOTO_STAGING,
    STAGING,
};
use crate::apps::registry::{self, id_is_valid, MANIFEST, MANIFEST_MAX_BYTES};
use ::fs::{Errno, Kind, OpenFlags, Vfs};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

impl Session {
    pub(super) fn feed_upload<'d>(
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
        up.crc = crate::formats::crc32::update(up.crc, chunk);
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

    pub(super) fn staging_root(&self) -> &'static str {
        match self.kind {
            UploadKind::App => STAGING,
            UploadKind::Skin => crate::companion::skins::STAGING,
            UploadKind::Brain => crate::companion::brain::store::STAGING,
            UploadKind::Photo => PHOTO_STAGING,
        }
    }

    pub(super) fn begin(
        &mut self,
        vfs: &mut Vfs,
        id: &str,
        kind: UploadKind,
    ) -> Result<String, String> {
        self.completed_photo = None;
        if !id_is_valid(id) {
            return Err(format!("bad id {id}"));
        }
        if kind == UploadKind::App {
            crate::apps::lifecycle::uninstall::ensure_ready(vfs, id)?;
        }
        if kind == UploadKind::Photo {
            crate::apps::lifecycle::uninstall::ensure_ready(vfs, "photo")?;
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

    pub(super) fn file(
        &mut self,
        vfs: &mut Vfs,
        rel: &str,
        size: &str,
        crc: &str,
    ) -> Result<(), String> {
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
                || size as usize > crate::companion::brain::SOURCE_MAX)
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

    pub(super) fn commit_photo(&mut self, vfs: &mut Vfs, rel: &str) -> Result<String, String> {
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
        crate::apps::lifecycle::uninstall::ensure_ready(vfs, "photo")?;
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

    pub(super) fn commit(
        &mut self,
        vfs: &mut Vfs,
        id: &str,
        run: bool,
        out: &mut dyn FnMut(Effect),
    ) -> Result<String, String> {
        if self.kind != UploadKind::App || self.staging.as_deref() != Some(id) {
            return Err(format!("commit {id}: not staged"));
        }
        crate::apps::lifecycle::uninstall::ensure_ready(vfs, id)?;
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
