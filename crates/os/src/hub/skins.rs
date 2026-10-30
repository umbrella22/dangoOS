//! Skin validation, transactional replacement and durable selection on a VFS.
use super::{
    json::{self, Spanned, Value},
    push::crc32,
    registry,
};
use crate::fox::{
    skin::{Skin, MAX_BYTES},
    H, W,
};
use ::fs::{Errno, Kind, OpenFlags, Vfs};
use alloc::{
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};

pub const ROOT: &str = "/data/skins";
pub const STAGING: &str = "/data/skins/.incoming";
const SELECTED: &str = "/data/skins/.selected";
pub const DEFAULT_NAME: &str = "鸣潮 · 心月狐团子";

// Existing NAND packages retain their stable IDs and saved selection when
// bundled characters are renamed. Custom packages keep their manifest names.
fn display_name<'a>(id: &str, manifest_name: &'a str) -> &'a str {
    match id {
        "default" => DEFAULT_NAME,
        "eclipse" => "玄灵 · 月蚀",
        "normal" => "玄灵 · 常态",
        "black-blue" => "智谱团子",
        _ => manifest_name,
    }
}

pub struct Loaded {
    pub id: String,
    pub skin: Rc<Skin>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub name: String,
}
/// Lightweight catalog: opening Settings reads names, never every image blob.
/// Full validation remains part of selecting and loading one resource.
pub fn choices(v: &mut Vfs) -> Result<Vec<Choice>, String> {
    let mut choices = alloc::vec![Choice { id: "default".into(), name: DEFAULT_NAME.into() }];
    let entries = match v.read_dir(ROOT) {
        Ok(entries) => entries,
        Err(Errno::NOENT) => return Ok(choices),
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries {
        if entry.meta.kind != Kind::Dir || valid_id(&entry.name).is_err() { continue; }
        let manifest = read(v, &format!("{ROOT}/{}/skin.json", entry.name), 4096)
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|_| "skin manifest JSON".into()));
        let Ok(manifest) = manifest else { continue; };
        let Some(name) = manifest["name"].as_str() else { continue; };
        if manifest["id"].as_str() != Some(entry.name.as_str()) || name.is_empty()
            || name.len() > 96 || name.chars().any(char::is_control) { continue; }
        let name = display_name(&entry.name, name).into();
        choices.push(Choice { id: entry.name, name });
    }
    let order = |id: &str| match id { "eclipse" => 0, "normal" => 1, "black-blue" => 2, _ => 3 };
    choices[1..].sort_by(|a, b| order(&a.id).cmp(&order(&b.id)).then_with(|| a.id.cmp(&b.id)));
    Ok(choices)
}
fn valid_id(id: &str) -> Result<(), String> {
    if !registry::id_is_valid(id) || id == "default" {
        Err("invalid/reserved skin id".into())
    } else {
        Ok(())
    }
}
fn num(value: Option<&Spanned>) -> Result<u32, String> {
    match value.map(|v| &v.value) {
        Some(Value::Number(n))
            if *n >= 0.0 && *n <= u32::MAX as f64 && *n == (*n as u32) as f64 =>
        {
            Ok(*n as u32)
        }
        _ => Err("skin manifest integer".into()),
    }
}
fn text<'a>(v: &'a Spanned, k: &str) -> Result<&'a str, String> {
    match v.get(k).map(|v| &v.value) {
        Some(Value::Str(s)) => Ok(s),
        _ => Err(format!("skin manifest {k}")),
    }
}
fn read(v: &mut Vfs, path: &str, max: usize) -> Result<Vec<u8>, String> {
    let meta = v.stat(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.kind != Kind::File || meta.size as usize > max {
        return Err(format!("{path}: file type/size"));
    }
    v.read_file(path).map_err(|e| format!("{path}: {e}"))
}
pub fn load_dir(v: &mut Vfs, dir: &str, id: &str) -> Result<Rc<Skin>, String> {
    valid_id(id)?;
    let bytes = read(v, &format!("{dir}/skin.json"), 4096)?;
    let manifest = json::parse(core::str::from_utf8(&bytes).map_err(|_| "skin manifest UTF-8")?)?;
    let Value::Object(fields) = &manifest.value else {
        return Err("skin manifest object".into());
    };
    let allowed = [
        "schema_version",
        "id",
        "name",
        "version",
        "canvas",
        "binary_bytes",
        "binary_crc32",
    ];
    if fields.len() != allowed.len()
        || allowed
            .iter()
            .any(|k| fields.iter().filter(|(n, _)| n == k).count() != 1)
    {
        return Err("skin manifest missing/duplicate/unknown fields".into());
    }
    if num(manifest.get("schema_version"))? != 1 || text(&manifest, "id")? != id {
        return Err("skin manifest version/id mismatch".into());
    }
    for (key, max) in [("name", 96), ("version", 32)] {
        let s = text(&manifest, key)?;
        if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
            return Err(format!("skin manifest {key} length/control"));
        }
    }
    let Some(Value::Array(canvas)) = manifest.get("canvas").map(|v| &v.value) else {
        return Err("skin manifest canvas".into());
    };
    if canvas.len() != 2 || num(canvas.first())? != W as u32 || num(canvas.get(1))? != H as u32 {
        return Err("skin manifest canvas mismatch".into());
    }
    let binary = read(v, &format!("{dir}/fox.bin"), MAX_BYTES)?;
    if num(manifest.get("binary_bytes"))? as usize != binary.len()
        || num(manifest.get("binary_crc32"))? != crc32(&binary)
    {
        return Err("skin manifest binary length/checksum mismatch".into());
    }
    Skin::parse(binary)
}
fn remove_if_present(v: &mut Vfs, path: &str) -> Result<(), String> {
    match v.remove_tree(path) {
        Ok(()) | Err(Errno::NOENT) => Ok(()),
        Err(e) => Err(format!("{path}: {e}")),
    }
}
/// Recover an interrupted two-rename replacement. A valid old copy is never
/// removed until a valid target exists; bad/missing target restores the backup.
pub fn recover(v: &mut Vfs, id: &str) -> Result<(), String> {
    valid_id(id)?;
    let target = format!("{ROOT}/{id}");
    let backup = format!("{ROOT}/.previous/{id}");
    match v.stat(&backup) {
        Ok(_) => {}
        Err(Errno::NOENT) => return Ok(()),
        Err(e) => return Err(format!("skin backup stat: {e}")),
    }
    if load_dir(v, &target, id).is_ok() {
        return remove_if_present(v, &backup);
    }
    load_dir(v, &backup, id).map_err(|e| format!("skin recovery kept backup: {e}"))?;
    remove_if_present(v, &target)?;
    v.rename(&backup, &target)
        .map_err(|e| format!("skin restore: {e}"))
}
pub fn selected(v: &mut Vfs) -> Result<String, String> {
    match v.stat(SELECTED) {
        Err(Errno::NOENT) => Ok("default".into()),
        Err(e) => Err(format!("selection: {e}")),
        Ok(_) => {
            let b = read(v, SELECTED, 63)?;
            let id = core::str::from_utf8(&b).map_err(|_| "selection UTF-8")?;
            if id != "default" {
                valid_id(id)?;
            }
            Ok(id.into())
        }
    }
}
fn persist(v: &mut Vfs, id: &str) -> Result<(), String> {
    v.create_dir_all(ROOT)
        .map_err(|e| format!("skin root: {e}"))?;
    let temp = format!("{SELECTED}.new");
    let fd = v
        .open(&temp, OpenFlags::WRITE)
        .map_err(|e| format!("selection open: {e}"))?;
    let result = (|| {
        let n = v.write(fd, id.as_bytes())?;
        if n != id.len() {
            return Err(Errno::NOSPC);
        }
        v.fsync(fd)
    })();
    let close = v.close(fd);
    result
        .and(close)
        .map_err(|e| format!("selection write: {e}"))?;
    v.rename(&temp, SELECTED)
        .map_err(|e| format!("selection rename: {e}"))
}
pub fn select(v: &mut Vfs, id: &str) -> Result<(), String> {
    if id != "default" {
        recover(v, id)?;
        load_dir(v, &format!("{ROOT}/{id}"), id)?;
    }
    persist(v, id)
}
/// Validate and persist a UI selection once, retaining the validated resource
/// for activation. The previous live skin survives any read or write failure.
pub fn select_loaded(v: &mut Vfs, id: &str, log: &mut dyn FnMut(&str)) -> Result<Loaded, String> {
    let skin = if id == "default" {
        default_skin(v, log)
    } else {
        recover(v, id)?;
        load_dir(v, &format!("{ROOT}/{id}"), id)?
    };
    persist(v, id)?;
    Ok(Loaded { id: id.into(), skin })
}
fn default_skin(v: &mut Vfs, log: &mut dyn FnMut(&str)) -> Rc<Skin> {
    let loaded = read(v, "/system/skins/default.bin", MAX_BYTES).and_then(Skin::parse);
    match loaded {
        Ok(skin) => skin,
        Err(reason) => {
            log(&format!("skin system default unavailable: {reason}; using recovery art"));
            Skin::builtin()
        }
    }
}
pub fn restore(v: &mut Vfs, log: &mut dyn FnMut(&str)) -> Loaded {
    let result = (|| {
        let id = selected(v)?;
        if id == "default" {
            return Ok(Loaded {
                id,
                skin: default_skin(v, log),
            });
        }
        recover(v, &id)?;
        let skin = load_dir(v, &format!("{ROOT}/{id}"), &id)?;
        Ok::<_, String>(Loaded { id, skin })
    })();
    result.unwrap_or_else(|reason| {
        log(&format!("skin fallback default: {reason}"));
        Loaded {
            id: "default".into(),
            skin: default_skin(v, log),
        }
    })
}
pub fn commit(v: &mut Vfs, id: &str) -> Result<(), String> {
    valid_id(id)?;
    let staged = format!("{STAGING}/{id}");
    load_dir(v, &staged, id)?; // complete validation before touching installed data
    recover(v, id)?;
    let target = format!("{ROOT}/{id}");
    let backup = format!("{ROOT}/.previous/{id}");
    v.create_dir_all(&format!("{ROOT}/.previous"))
        .map_err(|e| e.to_string())?;
    let old = match v.stat(&target) {
        Ok(_) => true,
        Err(Errno::NOENT) => false,
        Err(e) => return Err(e.to_string()),
    };
    if old {
        v.rename(&target, &backup)
            .map_err(|e| format!("skin backup: {e}"))?;
    }
    if let Err(e) = v.rename(&staged, &target) {
        if old {
            v.rename(&backup, &target)
                .map_err(|r| format!("skin rename {e}; rollback pending {r}"))?;
        }
        return Err(format!("skin rename: {e}"));
    }
    // Installed new package is complete. Cleanup is recoverable and must not
    // turn a committed replacement into a misleading protocol failure.
    let _ = remove_if_present(v, &backup);
    Ok(())
}
pub fn remove(v: &mut Vfs, id: &str) -> Result<(), String> {
    valid_id(id)?;
    if selected(v)? == id {
        persist(v, "default")?;
    }
    remove_if_present(v, &format!("{ROOT}/.previous/{id}"))?;
    v.remove_tree(&format!("{ROOT}/{id}"))
        .map_err(|e| format!("skin remove (selection may now be default): {e}"))
}
pub fn list(v: &mut Vfs) -> Result<String, String> {
    let mut ids = match v.read_dir(ROOT) {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.meta.kind == Kind::Dir && valid_id(&e.name).is_ok())
            .map(|e| e.name)
            .collect::<Vec<_>>(),
        Err(Errno::NOENT) => Vec::new(),
        Err(e) => return Err(e.to_string()),
    };
    ids.sort();
    Ok(format!(
        "skin-ls default{}",
        ids.iter().map(|s| format!(" {s}")).collect::<String>()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fox::{skin::fixture, FoxPose, FoxRenderer, Scene, PIXELS};
    use ::fs::{Fs, Geometry, RamDevice};
    use alloc::boxed::Box;
    use alloc::vec;
    fn disk() -> Vfs {
        let mut v = Vfs::new();
        v.mount(
            "data",
            Fs::mount_or_format(Box::new(RamDevice::new(4096, 1024)), Geometry::flash(1024))
                .unwrap()
                .0,
        );
        v
    }
    fn install(v: &mut Vfs, id: &str, xor: u16) {
        let dir = format!("{STAGING}/{id}");
        v.create_dir_all(&dir).unwrap();
        let binary = fixture(xor);
        let manifest = format!(
            r#"{{"schema_version":1,"id":"{id}","name":"Test","version":"1","canvas":[352,336],"binary_bytes":{},"binary_crc32":{}}}"#,
            binary.len(),
            crc32(&binary)
        );
        v.write_file(&format!("{dir}/fox.bin"), &binary).unwrap();
        v.write_file(&format!("{dir}/skin.json"), manifest.as_bytes())
            .unwrap();
        commit(v, id).unwrap();
    }
    fn pixels(skin: Rc<Skin>) -> Vec<u16> {
        let mut out = vec![0; PIXELS];
        FoxRenderer::new().render(&Scene::with_skin(FoxPose::Idle, skin), &mut out);
        out
    }
    #[test]
    fn renamed_bundled_characters_keep_existing_nand_ids_and_custom_names() {
        let mut v = disk();
        for (id, name) in [("eclipse", "月蚀"), ("normal", "常态"),
            ("black-blue", "黑发蓝挑染"), ("custom", "自定义团子")] {
            let path = format!("{ROOT}/{id}");
            v.create_dir_all(&path).unwrap();
            let manifest = serde_json::json!({"id": id, "name": name});
            v.write_file(&format!("{path}/skin.json"), manifest.to_string().as_bytes()).unwrap();
        }
        persist(&mut v, "black-blue").unwrap();
        let before = v.read_file(&format!("{ROOT}/black-blue/skin.json")).unwrap();
        let catalog = choices(&mut v).unwrap();
        assert_eq!(catalog.iter().map(|c| (c.id.as_str(), c.name.as_str())).collect::<Vec<_>>(), [
            ("default", "鸣潮 · 心月狐团子"), ("eclipse", "玄灵 · 月蚀"),
            ("normal", "玄灵 · 常态"), ("black-blue", "智谱团子"), ("custom", "自定义团子"),
        ]);
        assert_eq!(selected(&mut v).unwrap(), "black-blue");
        assert_eq!(v.read_file(&format!("{ROOT}/black-blue/skin.json")).unwrap(), before);
    }
    #[test]
    fn prepared_selection_reuses_validated_pixels_and_preserves_the_previous_choice_on_failure() {
        let mut v = disk();
        install(&mut v, "one", 0x07e0);
        let loaded = select_loaded(&mut v, "one", &mut |_| {}).unwrap();
        assert_eq!(loaded.id, "one");
        assert_eq!(selected(&mut v).unwrap(), "one");
        let expected = pixels(Rc::clone(&loaded.skin));
        // Activation uses its owned bytes even if storage is unavailable later.
        v.remove_tree(&format!("{ROOT}/one")).unwrap();
        assert_eq!(pixels(loaded.skin), expected);
        assert!(select_loaded(&mut v, "missing", &mut |_| {}).is_err());
        assert_eq!(selected(&mut v).unwrap(), "one");
        v.create_dir_all(&format!("{SELECTED}.new")).unwrap();
        assert!(select_loaded(&mut v, "default", &mut |_| {}).is_err());
        assert_eq!(selected(&mut v).unwrap(), "one");
    }
    #[test]
    #[ignore = "requires private assets/dango-family artwork"]
    fn family_catalog_persists_a_validated_choice_and_reloads_that_character() {
        let mut v = disk();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dango-family/skins");
        for id in ["eclipse", "normal", "black-blue"] {
            let manifest = std::fs::read(root.join(id).join("skin.json")).unwrap();
            let binary = std::fs::read(root.join(id).join("fox.bin")).unwrap();
            let path = format!("{ROOT}/{id}");
            v.create_dir_all(&path).unwrap();
            v.write_file(&format!("{path}/skin.json"), &manifest).unwrap();
            v.write_file(&format!("{path}/fox.bin"), &binary).unwrap();
        }
        let catalog = choices(&mut v).unwrap();
        assert_eq!(catalog.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["鸣潮 · 心月狐团子", "玄灵 · 月蚀", "玄灵 · 常态", "智谱团子"]);
        select(&mut v, "normal").unwrap();
        assert_eq!(selected(&mut v).unwrap(), "normal");
        let normal = restore(&mut v, &mut |_| panic!("valid selection fell back"));
        assert_eq!(normal.id, "normal");
        let before = pixels(normal.skin);
        v.write_file(&format!("{ROOT}/eclipse/fox.bin"), b"broken").unwrap();
        assert!(select(&mut v, "eclipse").is_err());
        assert_eq!(selected(&mut v).unwrap(), "normal");
        assert_eq!(pixels(restore(&mut v, &mut |_| {}).skin), before);
    }
    #[test]
    fn selection_cold_load_replacement_same_id_and_delete_return_to_builtin() {
        let mut v = disk();
        install(&mut v, "recolor", 0x7800);
        select(&mut v, "recolor").unwrap();
        let old = restore(&mut v, &mut |_| panic!("unexpected fallback"));
        let before = pixels(old.skin);
        assert_eq!(old.id, "recolor");
        assert_eq!(selected(&mut v).unwrap(), "recolor");
        // A new restore has no RAM selection state: it reads the durable id.
        assert_eq!(pixels(restore(&mut v, &mut |_| {}).skin), before);
        install(&mut v, "recolor", 0x07e0);
        assert_ne!(pixels(restore(&mut v, &mut |_| {}).skin), before);
        remove(&mut v, "recolor").unwrap();
        assert_eq!(selected(&mut v).unwrap(), "default");
        assert_eq!(
            pixels(restore(&mut v, &mut |_| {}).skin),
            pixels(Skin::builtin())
        );
    }
    #[test]
    fn bad_package_and_failed_selection_write_preserve_installed_selection() {
        let mut v = disk();
        install(&mut v, "good", 0);
        select(&mut v, "good").unwrap();
        v.create_dir_all(&format!("{SELECTED}.new")).unwrap();
        assert!(select(&mut v, "default").is_err());
        assert_eq!(selected(&mut v).unwrap(), "good");
        let dir = format!("{STAGING}/good");
        v.create_dir_all(&dir).unwrap();
        v.write_file(&format!("{dir}/skin.json"), b"{}").unwrap();
        assert!(commit(&mut v, "good").is_err());
        assert_eq!(
            restore(&mut v, &mut |_| panic!("old package lost")).id,
            "good"
        );
    }
    #[test]
    fn missing_corrupt_and_invalid_identity_fall_back_with_reasons() {
        let mut v = disk();
        install(&mut v, "good", 0);
        select(&mut v, "good").unwrap();
        v.write_file(&format!("{ROOT}/good/fox.bin"), b"broken")
            .unwrap();
        let mut lines = Vec::new();
        assert_eq!(
            restore(&mut v, &mut |line| lines.push(line.to_string())).id,
            "default"
        );
        assert!(lines[0].contains("length/checksum"));
        v.remove_tree(&format!("{ROOT}/good")).unwrap();
        lines.clear();
        assert_eq!(
            restore(&mut v, &mut |l| lines.push(l.to_string())).id,
            "default"
        );
        assert!(lines[0].contains("not found"));
        for id in ["../bad", "white space", "", "bad/id", "a.b"] {
            assert!(select(&mut v, id).is_err());
            assert!(commit(&mut v, id).is_err());
        }
    }
    #[test]
    fn backup_recovers_after_interrupted_directory_swap_and_preserves_valid_old_data() {
        let mut v = disk();
        install(&mut v, "good", 0x7800);
        select(&mut v, "good").unwrap();
        let expected = pixels(restore(&mut v, &mut |_| {}).skin);
        v.rename(&format!("{ROOT}/good"), &format!("{ROOT}/.previous/good"))
            .unwrap();
        assert_eq!(pixels(restore(&mut v, &mut |_| {}).skin), expected);
        v.rename(&format!("{ROOT}/good"), &format!("{ROOT}/.previous/good"))
            .unwrap();
        v.create_dir_all(&format!("{ROOT}/good")).unwrap();
        v.write_file(&format!("{ROOT}/good/skin.json"), b"{}")
            .unwrap();
        assert_eq!(pixels(restore(&mut v, &mut |_| {}).skin), expected);
    }
    #[test]
    fn skin_protocol_upload_select_interruption_and_bad_replacement_do_not_touch_apps_or_active_skin(
    ) {
        use crate::hub::push::{Effect, Session};
        fn command(v: &mut Vfs, s: &mut Session, line: &str) -> Vec<Effect> {
            let mut effects = Vec::new();
            s.feed(v, format!("@mosaico {line}\n").as_bytes(), &mut |e| {
                effects.push(e)
            });
            effects
        }
        fn file(v: &mut Vfs, s: &mut Session, name: &str, b: &[u8]) {
            let effects = command(v, s, &format!("file {name} {} {:08x}", b.len(), crc32(b)));
            assert!(matches!(&effects[0],Effect::Reply(t) if t.starts_with("@ok ready")));
            let mut effects = Vec::new();
            for chunk in b.chunks(997) {
                s.feed(v, chunk, &mut |e| effects.push(e));
            }
            assert!(matches!(&effects[0],Effect::Reply(t) if t.starts_with("@ok file")));
        }
        let mut v = disk();
        let mut session = Session::new();
        let b = fixture(0x7800);
        let m = format!(
            r#"{{"schema_version":1,"id":"test","name":"Test","version":"1","canvas":[352,336],"binary_bytes":{},"binary_crc32":{}}}"#,
            b.len(),
            crc32(&b)
        );
        command(&mut v, &mut session, "skin-begin test");
        file(&mut v, &mut session, "skin.json", m.as_bytes());
        file(&mut v, &mut session, "fox.bin", &b);
        assert!(command(&mut v, &mut session, "skin-commit test").contains(&Effect::SkinsChanged));
        command(&mut v, &mut session, "skin-select test");
        assert_eq!(selected(&mut v).unwrap(), "test");
        assert!(v.stat("/data/apps/test").is_err());
        command(&mut v, &mut session, "skin-begin test");
        command(&mut v, &mut session, "file fox.bin 100 00000000");
        session.feed(&mut v, b"half", &mut |_| {});
        session.reset(&mut v);
        assert_eq!(
            restore(&mut v, &mut |_| panic!(
                "interruption damaged installed skin"
            ))
            .id,
            "test"
        );
        command(&mut v, &mut session, "skin-begin test");
        file(&mut v, &mut session, "skin.json", b"{}");
        assert!(command(&mut v, &mut session, "skin-commit test")
            .iter()
            .any(|e| matches!(e,Effect::Reply(t) if t.starts_with("@err"))));
        assert_eq!(
            restore(&mut v, &mut |_| panic!(
                "bad replacement damaged installed skin"
            ))
            .id,
            "test"
        );
        assert!(command(&mut v, &mut session, "skin-rm test").contains(&Effect::SkinsChanged));
        assert_eq!(selected(&mut v).unwrap(), "default");
    }
}
