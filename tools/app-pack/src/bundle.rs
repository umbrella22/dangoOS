//! Standalone application delivery. Reuses the runtime manifest and validation.
use crate::{
    image::{inventory, Entry, Result, Tree},
    Package,
};
use serde_json::{json, Value};
use std::{fs, io::Write, path::Path};

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().ok_or("missing parent")?).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}
fn write_tree(root: &Path, tree: &Tree) -> Result<()> {
    for (name, entry) in tree {
        match entry {
            Entry::Directory => fs::create_dir_all(root.join(name)).map_err(|e| e.to_string())?,
            Entry::File(bytes) => write_new(&root.join(name), bytes)?,
        }
    }
    Ok(())
}
pub fn export(package: &Package, out: &Path) -> Result<Value> {
    package.validate()?;
    validate_transport(&package.files)?;
    fs::create_dir(out).map_err(|e| e.to_string())?;
    write_tree(&out.join("payload"), &package.files)?;
    write_tree(&out.join("documentation"), &package.documentation)?;
    let report = json!({"schema":1, "kind":"dango-lua-application", "id":package.id,
        "files":inventory(&package.files), "documentation":inventory(&package.documentation)});
    write_new(
        &out.join("bundle.json"),
        &serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )?;
    verify(out)?;
    Ok(report)
}
pub fn verify(root: &Path) -> Result<Value> {
    let report: Value =
        serde_json::from_slice(&fs::read(root.join("bundle.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    crate::image::require(
        report["schema"] == 1 && report["kind"] == "dango-lua-application",
        "unsupported application bundle",
    )?;
    let id = report["id"].as_str().ok_or("bundle id missing")?;
    let mut files = Tree::new();
    super::collect(&root.join("payload"), Path::new(""), &mut files)?;
    let mut documentation = Tree::new();
    if root.join("documentation").exists() {
        super::collect(
            &root.join("documentation"),
            Path::new(""),
            &mut documentation,
        )?;
    }
    let package = Package {
        id: id.into(),
        files,
        documentation,
    };
    package.validate()?;
    validate_transport(&package.files)?;
    crate::image::require(
        inventory(&package.files) == report["files"]
            && inventory(&package.documentation) == report["documentation"],
        "application bundle inventory mismatch",
    )?;
    Ok(report)
}

fn validate_transport(files: &Tree) -> Result<()> {
    for (path, entry) in files {
        crate::image::require(
            os::console::protocol::rel_is_safe(path),
            "resource path cannot be transferred by USB protocol v1",
        )?;
        if let Entry::File(bytes) = entry {
            crate::image::require(
                bytes.len() <= os::console::protocol::FILE_MAX as usize,
                "resource exceeds USB protocol v1 file limit",
            )?;
        }
    }
    Ok(())
}
