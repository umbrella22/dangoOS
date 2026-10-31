//! Validate distributable NAND skins through the device's manifest and decoder.
use fs::{Fs, Geometry, RamDevice, Vfs};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs as host, path::Path};
use {crate::image::require, crate::image::sha, crate::image::Result};

pub fn check(root: &Path) -> Result<Vec<Value>> {
    let mut entries = host::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    let mut skins = Vec::new();
    for entry in entries {
        let id = entry
            .file_name()
            .to_str()
            .ok_or("non-UTF-8 skin id")?
            .to_owned();
        if id.starts_with('.') {
            continue;
        }
        require(
            entry.file_type().map_err(|e| e.to_string())?.is_dir()
                && os::apps::registry::id_is_valid(&id)
                && id != "default",
            "invalid skin directory",
        )?;
        let directory = entry.path();
        let mut names = BTreeSet::new();
        for file in host::read_dir(&directory).map_err(|e| e.to_string())? {
            let file = file.map_err(|e| e.to_string())?;
            require(
                file.file_type().map_err(|e| e.to_string())?.is_file(),
                "skin resource must be a file",
            )?;
            names.insert(
                file.file_name()
                    .to_str()
                    .ok_or("non-UTF-8 skin filename")?
                    .to_owned(),
            );
        }
        require(
            names == BTreeSet::from(["fox.bin".into(), "skin.json".into()]),
            "skin directory needs exactly fox.bin and skin.json",
        )?;
        let binary = host::read(directory.join("fox.bin")).map_err(|e| e.to_string())?;
        let manifest = host::read(directory.join("skin.json")).map_err(|e| e.to_string())?;
        require(
            binary.len() <= os::companion::render::skin::MAX_BYTES && manifest.len() <= 4096,
            "skin resource exceeds device file limit",
        )?;
        // A fresh host-only filesystem exercises exactly the loading path
        // used after installation, including every compressed image stream.
        let blocks = (binary.len() + manifest.len()).div_ceil(4096) as u32 + 32;
        let volume = Fs::mount_or_format(
            Box::new(RamDevice::new(4096, blocks)),
            Geometry::flash(blocks),
        )
        .map_err(|e| e.to_string())?
        .0;
        let mut vfs = Vfs::new();
        vfs.mount("data", volume);
        let path = format!("/data/skins/{id}");
        vfs.create_dir_all(&path).map_err(|e| e.to_string())?;
        vfs.write_file(&format!("{path}/skin.json"), &manifest)
            .map_err(|e| e.to_string())?;
        vfs.write_file(&format!("{path}/fox.bin"), &binary)
            .map_err(|e| e.to_string())?;
        os::companion::skins::load_dir(&mut vfs, &path, &id)?;
        let metadata: Value = serde_json::from_slice(&manifest).map_err(|e| e.to_string())?;
        skins.push(json!({
            "id": id, "name": metadata["name"], "version": metadata["version"],
            "files": {
                "fox.bin": {"bytes":binary.len(), "sha256":sha(&binary)},
                "skin.json": {"bytes":manifest.len(), "sha256":sha(&manifest)},
            },
        }));
    }
    require(!skins.is_empty(), "no NAND skins selected")?;
    Ok(skins)
}
