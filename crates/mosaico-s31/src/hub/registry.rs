//! The Lua app registry: `launcher.json` manifests found under the app
//! roots, with the rules of esp-claw's `app_registry` (`parse_manifest`,
//! `load_root`).
//!
//! Roots, in priority order (an id found in an earlier root shadows the same
//! id later on, as the official registry does):
//!
//! 1. `/data/apps`: apps pushed over USB (`@mosaico` protocol).
//! 2. `/system/apps`: the factory apps on the official read-only partition.
//! 3. The apps bundled in the firmware (Snake, 2048).
//!
//! A manifest is accepted when `schema_version` is 1, `id` is valid and
//! equals the directory name, `entry` is a relative `.lua` path to an existing
//! file, `icon` (optional) is a relative `.jpg`/`.jpeg` path, `display_name`
//! (optional) is a non-empty string, `args` (optional) is an object, `order`
//! (optional) is an integer and `visible` (optional) a boolean. Invisible apps
//! are skipped, like the official launcher does.
//!
//! On top of that the entry script is checked for modules this firmware does
//! not have (`lvgl`, `image`, `thread`, ...): such apps are listed but marked
//! unavailable instead of failing on start.

use alloc::{boxed::Box, format, string::String, vec::Vec};

use mosaico_fs::{Kind, Vfs};

use super::json::{self, Value};

/// `launcher.json`, as the official registry names it.
pub const MANIFEST: &str = "launcher.json";
/// The official limit on manifest size.
pub const MANIFEST_MAX_BYTES: u32 = 4096;
/// The official limit on registry entries.
pub const MAX_APPS: usize = 64;
/// Longest app id (`APP_REGISTRY_ID_MAX_LEN`).
pub const ID_MAX_LEN: usize = 63;
/// The app roots on the file system, highest priority first.
pub const ROOTS: [(&str, Origin); 2] = [
    ("/data/apps", Origin::Data),
    ("/system/apps", Origin::System),
];
/// Directories `require` searches after the app's own (esp-claw
/// `app_capabilities.c` adds `<system>/scripts/builtin` and its `lib`).
pub const SHARED_LUA_DIRS: [&str; 2] = ["/system/scripts/builtin", "/system/scripts/builtin/lib"];
/// Lua script size checked for module use (larger scripts are checked in
/// their first part only).
const SCAN_MAX_BYTES: u32 = 256 * 1024;
/// Side of the launcher icon box, px.
pub const ICON_BOX: u16 = 96;

/// Modules every app gets: the Lua standard libraries and the esp-claw
/// modules `mosaico-lua` provides.
pub const PROVIDED_MODULES: [&str; 22] = [
    "string",
    "table",
    "math",
    "io",
    "os",
    "coroutine",
    "utf8",
    "debug",
    "package",
    "display",
    "delay",
    "system",
    "json",
    "arg_schema",
    "storage",
    "imu",
    "magnetometer",
    "fuel_gauge",
    "lib_fuel_gauge",
    "haptic",
    "led_strip",
    "button",
];

/// Where an app comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Data,
    System,
    Builtin,
}

/// A Lua app compiled into the firmware (esp-claw-skills-lab, MIT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Snake,
    Game2048,
}

impl Builtin {
    pub const ALL: [Builtin; 2] = [Builtin::Snake, Builtin::Game2048];

    pub const fn manifest(self) -> &'static str {
        match self {
            Builtin::Snake => {
                include_str!("../../../mosaico-lua/vendor/skills-lab/snake/launcher.json")
            }
            Builtin::Game2048 => {
                include_str!("../../../mosaico-lua/vendor/skills-lab/game_2048/launcher.json")
            }
        }
    }

    /// The entry script, by its `entry` path.
    pub fn script(self, entry: &str) -> Option<&'static str> {
        match (self, entry) {
            (Builtin::Snake, "scripts/draw_snake.lua") => Some(include_str!(
                "../../../mosaico-lua/vendor/skills-lab/snake/scripts/draw_snake.lua"
            )),
            (Builtin::Game2048, "scripts/game_2048.lua") => Some(include_str!(
                "../../../mosaico-lua/vendor/skills-lab/game_2048/scripts/game_2048.lua"
            )),
            _ => None,
        }
    }

    pub const fn icon(self) -> &'static [u8] {
        match self {
            Builtin::Snake => {
                include_bytes!("../../../mosaico-lua/vendor/skills-lab/snake/assets/icon.jpg")
            }
            Builtin::Game2048 => {
                include_bytes!("../../../mosaico-lua/vendor/skills-lab/game_2048/assets/icon.jpg")
            }
        }
    }

    /// The launcher name: these two had Chinese names before the registry.
    pub const fn title(self) -> &'static str {
        match self {
            Builtin::Snake => "贪吃蛇",
            Builtin::Game2048 => "2048",
        }
    }
}

/// Where the entry script is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LuaSource {
    /// Compiled in: chunk name and source.
    Builtin {
        chunk: &'static str,
        source: &'static str,
    },
    /// An absolute VFS path.
    File(String),
}

/// What the Lua task runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaApp {
    pub id: String,
    pub source: LuaSource,
    /// `args` as JSON text (`{}` when absent).
    pub args_json: String,
}

/// A launcher icon, RGB565 (little-endian) for LVGL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icon {
    pub width: u16,
    pub height: u16,
    pub rgb565: &'static [u8],
}

/// One registry entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    pub app: LuaApp,
    pub title: String,
    pub order: i32,
    pub origin: Origin,
    /// The app directory (empty for built-in apps).
    pub dir: String,
    pub icon: Option<Icon>,
    /// Modules the entry requires that this firmware lacks. Non-empty means
    /// the app is listed but cannot run.
    pub missing: Vec<String>,
}

impl AppEntry {
    pub fn runnable(&self) -> bool {
        self.missing.is_empty()
    }
}

/// The scanned apps, in launcher order: runnable ones first, then by
/// `order` and title as the official launcher sorts them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub apps: Vec<AppEntry>,
}

impl Registry {
    /// Index of the app with `id`.
    pub fn find(&self, id: &str) -> Option<usize> {
        self.apps.iter().position(|a| a.app.id == id)
    }

    /// Only the built-in apps (no file system).
    pub fn builtin_only(icons: &mut IconCache) -> Registry {
        let mut reg = Registry::default();
        let mut log = |_: &str| {};
        reg.add_builtins(icons, &mut log);
        reg.sort();
        reg
    }

    /// Scan the roots on `vfs`, then add the built-in apps. `log` receives
    /// one line per skipped or shadowed app.
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
        reg.add_builtins(icons, log);
        reg.sort();
        reg
    }

    fn add_builtins(&mut self, icons: &mut IconCache, log: &mut dyn FnMut(&str)) {
        for b in Builtin::ALL {
            match builtin_entry(b, icons) {
                Ok(entry) if self.find(&entry.app.id).is_some() => {
                    log(&format!("built-in app {} is shadowed", entry.app.id));
                }
                Ok(entry) => self.apps.push(entry),
                Err(e) => log(&format!("built-in app {b:?}: {e}")),
            }
        }
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

/// `app_registry_id_is_valid`: 1..=63 of `[A-Za-z0-9_-]`.
pub fn id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX_LEN
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// `relative_path_is_valid`: non-empty, not absolute, no `..`, no `\`.
pub fn relative_path_is_valid(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains("..") && !path.contains('\\')
}

/// A parsed manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub entry: String,
    pub icon: Option<String>,
    pub display_name: Option<String>,
    pub args_json: String,
    pub order: i32,
    pub visible: bool,
}

/// Validate `launcher.json` for the app directory named `dir_name`.
pub fn parse_manifest(text: &str, dir_name: &str) -> Result<Manifest, String> {
    let root = json::parse(text)?;
    if !matches!(root.value, Value::Object(_)) {
        return Err("manifest is not an object".into());
    }
    let int = |v: &Value| match *v {
        Value::Number(n) if n == (n as i32) as f64 => Some(n as i32),
        _ => None,
    };
    let string = |key: &str| -> Result<Option<String>, String> {
        match root.get(key).map(|v| &v.value) {
            None => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("{key} is not a string")),
        }
    };
    match root.get("schema_version").and_then(|v| int(&v.value)) {
        Some(1) => {}
        _ => return Err("schema_version is not 1".into()),
    }
    let id = string("id")?.ok_or("id missing")?;
    if !id_is_valid(&id) || id != dir_name {
        return Err(format!("id {id:?} does not match the directory"));
    }
    let entry = string("entry")?.ok_or("entry missing")?;
    if !relative_path_is_valid(&entry) || !entry.ends_with(".lua") {
        return Err(format!("bad entry {entry:?}"));
    }
    let icon = string("icon")?;
    if let Some(icon) = &icon {
        if !relative_path_is_valid(icon) || !(icon.ends_with(".jpg") || icon.ends_with(".jpeg")) {
            return Err(format!("bad icon {icon:?}"));
        }
    }
    let display_name = string("display_name")?;
    if display_name.as_deref() == Some("") {
        return Err("display_name is empty".into());
    }
    let args_json = match root.get("args") {
        None => "{}".into(),
        Some(v) if matches!(v.value, Value::Object(_)) => text[v.start..v.end].into(),
        Some(_) => return Err("args is not an object".into()),
    };
    let order = match root.get("order") {
        None => 0,
        Some(v) => int(&v.value).ok_or("order is not an integer")?,
    };
    let visible = match root.get("visible").map(|v| &v.value) {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("visible is not a boolean".into()),
    };
    Ok(Manifest {
        id,
        entry,
        icon,
        display_name,
        args_json,
        order,
        visible,
    })
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
            source: LuaSource::File(entry),
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
    let Ok(fd) = vfs.open(path, mosaico_fs::OpenFlags::READ) else {
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

fn builtin_entry(b: Builtin, icons: &mut IconCache) -> Result<AppEntry, String> {
    // The id must match the directory name; built-ins have no directory,
    // so the manifest's own id stands in for it.
    let id = match json::parse(b.manifest())?.get("id").map(|v| &v.value) {
        Some(Value::Str(s)) => s.clone(),
        _ => return Err("id missing".into()),
    };
    let m = parse_manifest(b.manifest(), &id)?;
    let source = b.script(&m.entry).ok_or("entry not bundled")?;
    let chunk = m.entry.rsplit('/').next().unwrap_or("main.lua");
    let chunk: &'static str = Box::leak(String::from(chunk).into_boxed_str());
    Ok(AppEntry {
        title: b.title().into(),
        app: LuaApp {
            id: m.id,
            source: LuaSource::Builtin { chunk, source },
            args_json: m.args_json,
        },
        order: m.order,
        origin: Origin::Builtin,
        dir: String::new(),
        icon: icons.get(b.icon()),
        missing: Vec::new(),
    })
}

/// Modules `source` requires unconditionally that are neither provided nor
/// found by `local_module` (searching the app and shared directories).
/// `pcall(require, "x")` is optional and does not count; neither do
/// commented-out lines.
pub fn missing_modules(source: &str, local_module: &mut dyn FnMut(&str) -> bool) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for line in source.lines() {
        let code = line.split("--").next().unwrap_or("");
        let mut rest = code;
        while let Some(at) = rest.find("require") {
            let before_ok = rest[..at]
                .chars()
                .next_back()
                .map_or(true, |c| !(c.is_alphanumeric() || c == '_' || c == '.'));
            let after = rest[at + "require".len()..].trim_start();
            rest = &rest[at + "require".len()..];
            if !before_ok {
                continue;
            }
            let after = after
                .strip_prefix('(')
                .map(str::trim_start)
                .unwrap_or(after);
            let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let body = &after[1..];
            let Some(end) = body.find(quote) else {
                continue;
            };
            let name = &body[..end];
            if name.is_empty()
                || PROVIDED_MODULES.contains(&name)
                || missing.iter().any(|m| m == name)
            {
                continue;
            }
            if !local_module(name) {
                missing.push(name.into());
            }
        }
    }
    missing
}

/// Decoded icons, kept for the firmware's lifetime (LVGL images need
/// `'static` pixels). Keyed by content, so rescans reuse them.
#[derive(Default)]
pub struct IconCache {
    entries: Vec<(u64, Icon)>,
}

/// Most icons kept (each is at most `ICON_BOX`² × 2 bytes = 18 KiB).
const ICON_CACHE_MAX: usize = 48;

impl IconCache {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The icon for this JPEG, decoded and scaled to fit [`ICON_BOX`].
    pub fn get(&mut self, jpeg: &[u8]) -> Option<Icon> {
        let key = fnv1a(jpeg);
        if let Some((_, icon)) = self.entries.iter().find(|(k, _)| *k == key) {
            return Some(*icon);
        }
        if self.entries.len() >= ICON_CACHE_MAX {
            return None;
        }
        let img = mosaico_lua::jpeg::decode(jpeg).ok()?;
        let icon = scale_to_box(&img);
        self.entries.push((key, icon));
        Some(icon)
    }
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h ^ data.len() as u64
}

/// Nearest-neighbour scale: whole-number enlargement for small (pixel art)
/// icons, shrink to fit for large ones.
fn scale_to_box(img: &mosaico_lua::jpeg::Rgb888) -> Icon {
    let (w, h) = (img.width as u32, img.height as u32);
    let side = w.max(h);
    let (nw, nh) = if side <= ICON_BOX as u32 {
        let k = (ICON_BOX as u32 / side).max(1);
        (w * k, h * k)
    } else {
        (
            (w * ICON_BOX as u32 / side).max(1),
            (h * ICON_BOX as u32 / side).max(1),
        )
    };
    let mut out = Vec::with_capacity((nw * nh * 2) as usize);
    for y in 0..nh {
        let sy = y * h / nh;
        for x in 0..nw {
            let sx = x * w / nw;
            let p = &img.pixels[((sy * w + sx) * 3) as usize..][..3];
            let v = ((p[0] as u16 & 0xF8) << 8) | ((p[1] as u16 & 0xFC) << 3) | (p[2] as u16 >> 3);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    Icon {
        width: nw as u16,
        height: nh as u16,
        rgb565: Box::leak(out.into_boxed_slice()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaico_fs::{Fs, Geometry, RamDevice};

    fn vfs() -> Vfs {
        let mut v = Vfs::new();
        for (name, blocks) in [("system", 64), ("data", 64)] {
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

    fn put(v: &mut Vfs, path: &str, data: &str) {
        let dir = path.rsplit_once('/').unwrap().0;
        v.create_dir_all(dir).unwrap();
        v.write_file(path, data.as_bytes()).unwrap();
    }

    fn manifest(id: &str, extra: &str) -> String {
        format!(r#"{{"schema_version":1,"id":"{id}","entry":"scripts/main.lua"{extra}}}"#)
    }

    #[test]
    fn manifests_follow_the_official_rules() {
        let ok = parse_manifest(
            r#"{"schema_version":1,"id":"dino","entry":"scripts/dino.lua","display_name":"Dino",
                "icon":"assets/icon.jpg","args":{ "a" : [1] },"order":10,"visible":true}"#,
            "dino",
        )
        .unwrap();
        assert_eq!(ok.display_name.as_deref(), Some("Dino"));
        assert_eq!(ok.args_json, r#"{ "a" : [1] }"#);
        assert_eq!((ok.order, ok.visible), (10, true));
        let min = parse_manifest(&manifest("x", ""), "x").unwrap();
        assert_eq!(
            (min.args_json.as_str(), min.order, min.visible, min.icon),
            ("{}", 0, true, None)
        );
        for (bad, dir) in [
            (manifest("x", ""), "y"),
            (
                manifest("x", "").replace("\"schema_version\":1", "\"schema_version\":2"),
                "x",
            ),
            (
                manifest("x", "").replace("\"schema_version\":1", "\"schema_version\":1.5"),
                "x",
            ),
            (
                manifest("x", "").replace("scripts/main.lua", "/abs.lua"),
                "x",
            ),
            (
                manifest("x", "").replace("scripts/main.lua", "../up.lua"),
                "x",
            ),
            (
                manifest("x", "").replace("scripts/main.lua", "main.py"),
                "x",
            ),
            (manifest("x", r#","icon":"a.png""#), "x"),
            (manifest("x", r#","display_name":"""#), "x"),
            (manifest("x", r#","args":[]"#), "x"),
            (manifest("x", r#","order":1.5"#), "x"),
            (manifest("x", r#","visible":1"#), "x"),
            (manifest("a b", ""), "a b"),
            ("[]".into(), "x"),
            ("{".into(), "x"),
        ] {
            assert!(parse_manifest(&bad, dir).is_err(), "{bad}");
        }
        assert!(id_is_valid(&"a".repeat(63)) && !id_is_valid(&"a".repeat(64)));
    }

    #[test]
    fn module_check_sees_hard_requires_only() {
        let src = r#"
            local display = require("display")
            local lvgl, storage = require("lvgl"), require 'storage'
            local ok, button = pcall(require, "button")
            -- local net = require("net")
            local helper = require("helper")
            print("require(button) failed")
            local x = my_require("thing")
            local again = require("lvgl")
        "#;
        let missing = missing_modules(src, &mut |m| m == "helper");
        assert_eq!(missing, ["lvgl"]);
    }

    #[test]
    fn scan_merges_roots_with_shadowing_and_order() {
        let mut v = vfs();
        // /system: a runnable app, one needing lvgl, a hidden one, a broken one.
        put(
            &mut v,
            "/system/apps/dino/launcher.json",
            &manifest("dino", r#","display_name":"Dino","order":10"#),
        );
        put(
            &mut v,
            "/system/apps/dino/scripts/main.lua",
            "local d = require('display')",
        );
        put(
            &mut v,
            "/system/apps/album/launcher.json",
            &manifest("album", r#","order":12"#),
        );
        put(
            &mut v,
            "/system/apps/album/scripts/main.lua",
            "local l = require('lvgl')\nlocal i = require('image')",
        );
        put(
            &mut v,
            "/system/apps/hidden/launcher.json",
            &manifest("hidden", r#","visible":false"#),
        );
        put(&mut v, "/system/apps/hidden/scripts/main.lua", "");
        put(
            &mut v,
            "/system/apps/broken/launcher.json",
            &manifest("other", ""),
        );
        put(
            &mut v,
            "/system/apps/noentry/launcher.json",
            &manifest("noentry", ""),
        );
        put(
            &mut v,
            "/system/scripts/builtin/lib/ssd1306.lua",
            "return {}",
        );
        // /data: shadows dino, adds one that uses a shared library, and
        // replaces the built-in snake.
        put(
            &mut v,
            "/data/apps/dino/launcher.json",
            &manifest("dino", r#","display_name":"Dino 2","order":10"#),
        );
        put(&mut v, "/data/apps/dino/scripts/main.lua", "");
        put(
            &mut v,
            "/data/apps/oled/launcher.json",
            &manifest("oled", r#","order":-1"#),
        );
        put(
            &mut v,
            "/data/apps/oled/scripts/main.lua",
            "local s = require('ssd1306')",
        );
        put(
            &mut v,
            "/data/apps/snake/launcher.json",
            &manifest("snake", r#","display_name":"My Snake","order":20"#),
        );
        put(&mut v, "/data/apps/snake/scripts/main.lua", "");
        v.create_dir_all("/data/apps/.incoming/x").unwrap();

        let mut logs = Vec::new();
        let mut icons = IconCache::new();
        let reg = Registry::scan(&mut v, &mut icons, &mut |l| logs.push(String::from(l)));
        let ids: Vec<&str> = reg.apps.iter().map(|a| a.app.id.as_str()).collect();
        assert_eq!(
            ids,
            ["oled", "dino", "game_2048", "snake", "album"],
            "{logs:?}"
        );
        let dino = &reg.apps[reg.find("dino").unwrap()];
        assert_eq!((dino.title.as_str(), dino.origin), ("Dino 2", Origin::Data));
        assert_eq!(
            dino.app.source,
            LuaSource::File("/data/apps/dino/scripts/main.lua".into())
        );
        let album = &reg.apps[reg.find("album").unwrap()];
        assert_eq!(album.missing, ["lvgl", "image"]);
        assert!(!album.runnable() && reg.apps[0].runnable());
        let snake = &reg.apps[reg.find("snake").unwrap()];
        assert_eq!(
            (snake.title.as_str(), snake.origin),
            ("My Snake", Origin::Data)
        );
        let g = &reg.apps[reg.find("game_2048").unwrap()];
        assert_eq!(g.origin, Origin::Builtin);
        assert_eq!(
            g.app.args_json.split_whitespace().collect::<String>(),
            r#"{"run_time_ms":0}"#
        );
        assert!(matches!(
            g.app.source,
            LuaSource::Builtin {
                chunk: "game_2048.lua",
                ..
            }
        ));
        let icon = g.icon.expect("built-in icon decodes");
        assert!(icon.width <= ICON_BOX && icon.height <= ICON_BOX && icon.width >= 48);
        for needle in [
            "dino in /system/apps is shadowed",
            "broken",
            "noentry",
            "built-in app snake is shadowed",
        ] {
            assert!(
                logs.iter().any(|l| l.contains(needle)),
                "{needle}: {logs:?}"
            );
        }
        assert!(!logs.iter().any(|l| l.contains("hidden")), "{logs:?}");
    }

    #[test]
    fn builtins_without_a_file_system() {
        let reg = Registry::builtin_only(&mut IconCache::new());
        let ids: Vec<&str> = reg.apps.iter().map(|a| a.app.id.as_str()).collect();
        assert_eq!(ids, ["game_2048", "snake"]);
        assert_eq!(reg.apps[1].title, "贪吃蛇");
    }

    #[test]
    fn icons_scale_into_the_box_and_are_cached() {
        let img = mosaico_lua::jpeg::Rgb888 {
            width: 48,
            height: 24,
            pixels: alloc::vec![255; 48 * 24 * 3],
        };
        let i = scale_to_box(&img);
        assert_eq!((i.width, i.height, i.rgb565.len()), (96, 48, 96 * 48 * 2));
        assert_eq!(&i.rgb565[..2], &[0xFF, 0xFF]);
        let big = mosaico_lua::jpeg::Rgb888 {
            width: 200,
            height: 100,
            pixels: alloc::vec![0; 200 * 100 * 3],
        };
        let b = scale_to_box(&big);
        assert_eq!((b.width, b.height), (96, 48));
        let mut cache = IconCache::new();
        let a = cache.get(Builtin::Snake.icon()).unwrap();
        let again = cache.get(Builtin::Snake.icon()).unwrap();
        assert!(core::ptr::eq(a.rgb565, again.rgb565));
        assert!(cache.get(b"nope").is_none());
    }

    /// The factory apps on the device's `/system` partition image.
    #[test]
    fn official_system_image() {
        let Some(path) = std::env::var_os("MOSAICO_SYSTEM_IMAGE") else {
            return;
        };
        let image = std::fs::read(path).unwrap();
        let mut v = Vfs::new();
        let blocks = image.len() as u32 / 4096;
        let fs = Fs::mount(
            Box::new(RamDevice::from_image(image, 4096)),
            Geometry::flash(blocks),
            true,
        )
        .unwrap();
        v.mount("system", fs);
        let mut logs = Vec::new();
        let reg = Registry::scan(&mut v, &mut IconCache::new(), &mut |l| {
            logs.push(String::from(l))
        });
        let get = |id: &str| &reg.apps[reg.find(id).unwrap_or_else(|| panic!("{id}: {logs:?}"))];
        assert!(get("dino").runnable() && get("flappybird").runnable());
        assert_eq!(get("flappybird").title, "Flappy Bird");
        assert!(get("dino").icon.is_some());
        assert!(get("album_app").missing.contains(&String::from("lvgl")));
        assert!(!get("app_store").runnable());
        std::println!(
            "{:?}",
            reg.apps
                .iter()
                .map(|a| (&a.app.id, a.order, &a.missing))
                .collect::<Vec<_>>()
        );
    }
}
