//! Shared app management contract. Callers must stop/drop the VM before `execute`.
//! `.trash/<id>` is the durable intent: recovery always clears both body and data.
use super::registry::{id_is_valid, IconCache, Origin, Registry};
use alloc::{format, string::String};
use mosaico_fs::{Errno, Vfs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    id: [u8; 63],
    len: u8,
    pub origin: Origin,
}
impl Identity {
    pub fn new(id: &str, origin: Origin) -> Option<Self> {
        if !id_is_valid(id) {
            return None;
        }
        let mut bytes = [0; 63];
        bytes[..id.len()].copy_from_slice(id.as_bytes());
        Some(Self {
            id: bytes,
            len: id.len() as u8,
            origin,
        })
    }
    pub fn id(&self) -> &str {
        core::str::from_utf8(&self.id[..self.len as usize]).expect("validated identity")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Remove(Identity),
    Clear(Identity),
}
impl Operation {
    pub fn identity(self) -> Identity {
        match self {
            Self::Remove(i) | Self::Clear(i) => i,
        }
    }
    pub fn verb(self) -> &'static str {
        match self {
            Self::Remove(_) => "rm",
            Self::Clear(_) => "rmdata",
        }
    }
}
fn remove(vfs: &mut Vfs, path: &str) -> Result<(), String> {
    match vfs.remove_tree(path) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(e) => Err(format!("{path}: {e}")),
    }
}
pub fn ensure_ready(vfs: &mut Vfs, id: &str) -> Result<(), String> {
    if !id_is_valid(id) {
        return Err("invalid app id".into());
    }
    for root in ["/data/apps/.trash", "/data/apps/.clear"] {
        match vfs.stat(&format!("{root}/{id}")) {
            Ok(_) => return Err("app has pending recovery".into()),
            Err(Errno::NOENT) => {}
            Err(e) => return Err(format!("recovery check: {e}")),
        }
    }
    Ok(())
}
pub fn resolve(vfs: &mut Vfs, id: &str) -> Result<Identity, String> {
    ensure_ready(vfs, id)?;
    let reg = Registry::scan(vfs, &mut IconCache::new(), &mut |_| {});
    let i = reg.find(id).ok_or_else(|| format!("app {id} not found"))?;
    Identity::new(id, reg.apps[i].origin).ok_or_else(|| "invalid id".into())
}
/// Must run with exclusive management ownership, after the VM drop ack.
pub fn execute(vfs: &mut Vfs, op: Operation) -> Result<(), String> {
    let expected = op.identity();
    if resolve(vfs, expected.id())? != expected {
        return Err("app source changed; confirm again".into());
    }
    if matches!(op, Operation::Remove(_)) && expected.origin != Origin::Data {
        return Err("factory apps cannot be removed; use rmdata".into());
    }
    let id = expected.id();
    let trash = format!("/data/apps/.trash/{id}");
    if vfs.stat(&trash).is_ok() {
        return Err("pending recovery; retry after recovery".into());
    }
    // Clear-data uses a distinct durable marker so recovery preserves its body.
    let clear = matches!(op, Operation::Clear(_));
    let root = if clear {
        "/data/apps/.clear"
    } else {
        "/data/apps/.trash"
    };
    vfs.create_dir_all(root)
        .map_err(|e| format!("{root}: {e}"))?;
    let marker = format!("{root}/{id}");
    if clear {
        vfs.create_dir_all(&marker)
            .map_err(|e| format!("intent: {e}"))?;
    } else {
        vfs.rename(&format!("/data/apps/{id}"), &marker)
            .map_err(|e| format!("stage removal: {e}"))?;
    }
    finish(vfs, id, &marker)
}
fn finish(vfs: &mut Vfs, id: &str, marker: &str) -> Result<(), String> {
    remove(vfs, &format!("/data/apps/.incoming/{id}"))?;
    remove(vfs, &format!("/data/appdata/{id}"))?;
    remove(vfs, marker)
}
/// Run before scanning or starting apps. Errors retain durable pending intent.
pub fn recover(vfs: &mut Vfs) -> Result<(), String> {
    for root in ["/data/apps/.trash", "/data/apps/.clear"] {
        let entries = match vfs.read_dir(root) {
            Ok(e) => e,
            Err(Errno::NOENT) => continue,
            Err(e) => return Err(format!("recovery: {e}")),
        };
        for e in entries {
            if !id_is_valid(&e.name) {
                return Err("invalid recovery identity".into());
            }
            finish(vfs, &e.name, &format!("{root}/{}", e.name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaico_fs::{Fs, Geometry, RamDevice};
    fn vfs() -> Vfs {
        let mut v = Vfs::new();
        for name in ["data", "system"] {
            let fs = Fs::mount_or_format(
                alloc::boxed::Box::new(RamDevice::new(4096, 64)),
                Geometry::flash(64),
            )
            .unwrap()
            .0;
            v.mount(name, fs);
        }
        v
    }
    fn app(v: &mut Vfs, root: &str, id: &str) {
        let dir = format!("/{root}/apps/{id}");
        v.create_dir_all(&dir).unwrap();
        v.write_file(
            &format!("{dir}/launcher.json"),
            format!(
                r#"{{"schema_version":1,"id":"{id}","entry":"main.lua","display_name":"Test"}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        v.write_file(&format!("{dir}/main.lua"), b"return 1")
            .unwrap();
    }
    fn data(v: &mut Vfs, id: &str) {
        v.create_dir_all(&format!("/data/appdata/{id}")).unwrap();
        v.write_file(&format!("/data/appdata/{id}/save"), b"saved")
            .unwrap();
    }
    #[test]
    fn removes_shadow_and_data_but_preserves_factory_other_and_legacy() {
        let mut v = vfs();
        app(&mut v, "data", "same");
        app(&mut v, "system", "same");
        data(&mut v, "same");
        data(&mut v, "other");
        v.write_file("/data/legacy", b"unattributed").unwrap();
        let identity = resolve(&mut v, "same").unwrap();
        execute(&mut v, Operation::Remove(identity)).unwrap();
        assert_eq!(resolve(&mut v, "same").unwrap().origin, Origin::System);
        assert!(v.stat("/data/appdata/same").is_err());
        assert!(v.stat("/data/appdata/other/save").is_ok());
        assert!(v.stat("/data/legacy").is_ok());
        assert!(execute(&mut v, Operation::Remove(identity))
            .unwrap_err()
            .contains("source changed"));
    }
    #[test]
    fn system_builtin_and_rmdata_preserve_body() {
        let mut v = vfs();
        app(&mut v, "system", "factory");
        app(&mut v, "data", "user");
        for id in ["factory", "user", "snake"] {
            data(&mut v, id);
        }
        for (id, clear) in [("factory", true), ("user", true), ("snake", true)] {
            let identity = resolve(&mut v, id).unwrap();
            if identity.origin != Origin::Data {
                assert!(execute(&mut v, Operation::Remove(identity)).is_err());
            }
            execute(
                &mut v,
                if clear {
                    Operation::Clear(identity)
                } else {
                    Operation::Remove(identity)
                },
            )
            .unwrap();
            assert!(resolve(&mut v, id).is_ok());
            assert!(v.stat(&format!("/data/appdata/{id}")).is_err());
        }
    }
    #[test]
    fn recovery_finishes_interrupted_body_data_and_incoming_cleanup_idempotently() {
        let mut v = vfs();
        app(&mut v, "data", "pending");
        data(&mut v, "pending");
        v.create_dir_all("/data/apps/.trash").unwrap();
        v.rename("/data/apps/pending", "/data/apps/.trash/pending")
            .unwrap();
        v.create_dir_all("/data/apps/.incoming/pending").unwrap();
        v.write_file("/data/apps/.incoming/pending/main.lua", b"stale")
            .unwrap();
        recover(&mut v).unwrap();
        recover(&mut v).unwrap();
        for path in [
            "/data/apps/.trash/pending",
            "/data/apps/.incoming/pending",
            "/data/appdata/pending",
        ] {
            assert!(v.stat(path).is_err());
        }
    }
    #[test]
    fn clear_intent_recovery_preserves_data_app_body() {
        let mut v = vfs();
        app(&mut v, "data", "keep");
        data(&mut v, "keep");
        v.create_dir_all("/data/apps/.clear/keep").unwrap();
        recover(&mut v).unwrap();
        assert!(resolve(&mut v, "keep").is_ok());
        assert!(v.stat("/data/appdata/keep").is_err());
    }
    #[test]
    fn stable_identity_rejects_invalid_and_handles_longest_id() {
        for id in ["", "../x", "a/b", ".hidden"] {
            assert!(Identity::new(id, Origin::Data).is_none());
        }
        let id = "a".repeat(63);
        assert_eq!(Identity::new(&id, Origin::Data).unwrap().id(), id);
        assert!(Identity::new(&"a".repeat(64), Origin::Data).is_none());
    }
}

/// Management ownership outlives filesystem completion until the UI has
/// consumed the mandatory stop+rescan boundary. Navigation never overwrites it.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Executing,
    AwaitingRefresh,
}
impl Phase {
    pub fn acquire(&mut self) -> Result<(), &'static str> {
        if *self != Self::Idle {
            return Err("app management busy");
        }
        *self = Self::Executing;
        Ok(())
    }
    pub fn completed(&mut self) {
        assert_eq!(*self, Self::Executing);
        *self = Self::AwaitingRefresh;
    }
    pub fn acknowledge_refresh(&mut self) {
        assert_eq!(*self, Self::AwaitingRefresh);
        *self = Self::Idle;
    }
    pub fn busy(self) -> bool {
        self != Self::Idle
    }
}
#[cfg(test)]
mod phase_tests {
    use super::Phase;
    #[test]
    fn destructive_owner_survives_completion_until_refresh_and_rejects_second_owner() {
        let mut phase = Phase::default();
        phase.acquire().unwrap();
        assert!(phase.acquire().is_err());
        phase.completed();
        assert!(phase.busy());
        assert!(phase.acquire().is_err());
        phase.acknowledge_refresh();
        assert!(!phase.busy());
        phase.acquire().unwrap();
    }
}
