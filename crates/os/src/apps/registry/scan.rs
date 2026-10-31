//! Root traversal, precedence, file reads and launcher ordering.
use super::{
    id_is_valid, missing_modules, parse_manifest, AppEntry, IconCache, LuaApp, Origin, Registry,
    MANIFEST, MANIFEST_MAX_BYTES, MAX_APPS, ROOTS, SCAN_MAX_BYTES, SHARED_LUA_DIRS,
};
use ::fs::{Kind, Vfs};
use alloc::{format, string::String, vec::Vec};

impl Registry {
    /// Index of the app with `id`.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.apps.iter().position(|a| a.app.id == id)
    }

    /// Scan application roots on `vfs`. `log` receives one line per skipped
    /// or shadowed app. Missing mounts produce an empty Lua application list.
    pub fn scan(vfs: &mut Vfs, icons: &mut IconCache, log: &mut dyn FnMut(&str)) -> Registry {
        let mut reg = Registry::default();
        for (root, origin) in ROOTS {
            let Ok(entries) = vfs.read_dir(root) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .into_iter()
                .filter(|e| e.meta.kind == Kind::Dir && !e.name.starts_with('.'))
                .map(|e| e.name)
                .collect();
            names.sort();
            for name in names {
                if !id_is_valid(&name) {
                    continue;
                }
                if reg.find(&name).is_some() {
                    log(&format!("app {name} in {root} is shadowed"));
                    continue;
                }
                if reg.apps.len() >= MAX_APPS {
                    log(&format!("too many apps, {root}/{name} skipped"));
                    continue;
                }
                let dir = format!("{root}/{name}");
                match load_dir(vfs, &dir, &name, origin, icons) {
                    Ok(Some(entry)) => reg.apps.push(entry),
                    Ok(None) => {}
                    Err(e) => log(&format!("app {dir}: {e}")),
                }
            }
        }
        reg.sort();
        reg
    }

    /// Runnable apps first (the factory partition carries apps for modules
    /// this firmware lacks), each group by `order`, then title.
    fn sort(&mut self) {
        self.apps.sort_by(|a, b| {
            (!a.runnable())
                .cmp(&!b.runnable())
                .then(a.order.cmp(&b.order))
                .then_with(|| a.title.cmp(&b.title))
        });
    }
}

pub(crate) fn read_text(vfs: &mut Vfs, path: &str, max: u32) -> Result<String, String> {
    let meta = vfs.stat(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.kind != Kind::File {
        return Err(format!("{path} is not a file"));
    }
    if meta.size > max {
        return Err(format!("{path} is larger than {max} bytes"));
    }
    let bytes = vfs.read_file(path).map_err(|e| format!("{path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))
}

fn load_dir(
    vfs: &mut Vfs,
    dir: &str,
    name: &str,
    origin: Origin,
    icons: &mut IconCache,
) -> Result<Option<AppEntry>, String> {
    let text = read_text(vfs, &format!("{dir}/{MANIFEST}"), MANIFEST_MAX_BYTES)?;
    let m = parse_manifest(&text, name)?;
    if !m.visible {
        return Ok(None);
    }
    let entry = format!("{dir}/{}", m.entry);
    match vfs.stat(&entry) {
        Ok(meta) if meta.kind == Kind::File => {}
        _ => return Err(format!("entry {entry} missing")),
    }
    let source = scan_prefix(vfs, &entry);
    let entry_dir = entry.rsplit_once('/').map(|(d, _)| d).unwrap_or(dir);
    let missing = missing_modules(&source, &mut |module| {
        let rel = module.replace('.', "/");
        core::iter::once(entry_dir).chain(SHARED_LUA_DIRS).any(|d| {
            is_file(vfs, &format!("{d}/{rel}.lua")) || is_file(vfs, &format!("{d}/{rel}/init.lua"))
        })
    });
    let icon = m.icon.as_ref().and_then(|icon| {
        let path = format!("{dir}/{icon}");
        let bytes = vfs.read_file(&path).ok()?;
        icons.get(&bytes)
    });
    Ok(Some(AppEntry {
        title: m.display_name.unwrap_or_else(|| m.id.clone()),
        app: LuaApp {
            id: m.id,
            entry,
            args_json: m.args_json,
        },
        order: m.order,
        origin,
        dir: dir.into(),
        icon,
        missing,
    }))
}

fn is_file(vfs: &mut Vfs, path: &str) -> bool {
    matches!(vfs.stat(path), Ok(m) if m.kind == Kind::File)
}

/// The start of the entry script, for [`missing_modules`].
fn scan_prefix(vfs: &mut Vfs, path: &str) -> String {
    let Ok(fd) = vfs.open(path, ::fs::OpenFlags::READ) else {
        return String::new();
    };
    let mut buf = alloc::vec![0u8; SCAN_MAX_BYTES as usize];
    let mut len = 0;
    while len < buf.len() {
        match vfs.read(fd, &mut buf[len..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => len += n,
        }
    }
    let _ = vfs.close(fd);
    buf.truncate(len);
    String::from_utf8_lossy(&buf).into_owned()
}
