//! Patch an existing LittleFS system image without rebuilding its other files.
//! This host tool never connects to a board or formats an image.
use mosaico_fs::{BlockDevice, Errno, Fs, Geometry, Kind, OpenFlags, RamDevice, Vfs};
use mosaico_s31::hub::registry::{IconCache, Origin, Registry};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
};

const SCRIPT: &str = "/apps/album_app/scripts/album_app.lua";
const MANIFEST: &str = "/apps/album_app/launcher.json";
const ICON: &str = "/apps/album_app/assets/icon.jpg";
const ASSETS: &str = "/apps/album_app/assets";
const UPSTREAM_SCRIPT: &[u8] =
    include_bytes!("../../mosaico-lua/vendor/album_app/scripts/album_app.lua");
const UPSTREAM_MANIFEST: &[u8] = include_bytes!("../../mosaico-lua/vendor/album_app/launcher.json");
type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Shared(Arc<Mutex<RamDevice>>);
impl Shared {
    fn image(bytes: Vec<u8>) -> Self {
        Self(Arc::new(Mutex::new(RamDevice::from_image(bytes, 4096))))
    }
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().data.clone()
    }
}
impl BlockDevice for Shared {
    fn read(&mut self, b: u32, o: u32, bytes: &mut [u8]) -> std::result::Result<(), Errno> {
        self.0.lock().unwrap().read(b, o, bytes)
    }
    fn prog(&mut self, b: u32, o: u32, bytes: &[u8]) -> std::result::Result<(), Errno> {
        self.0.lock().unwrap().prog(b, o, bytes)
    }
    fn erase(&mut self, b: u32) -> std::result::Result<(), Errno> {
        self.0.lock().unwrap().erase(b)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Directory,
    File(Vec<u8>),
}
type Tree = BTreeMap<String, Entry>;
fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_file(fs: &mut Fs, path: &str) -> Result<Vec<u8>> {
    let size = fs.stat(path).map_err(|e| format!("stat {path}: {e}"))?.size as usize;
    let mut file = fs
        .open(path, OpenFlags::READ)
        .map_err(|e| format!("open {path}: {e}"))?;
    let mut bytes = vec![0; size];
    let mut offset = 0;
    while offset < size {
        let n = fs
            .read(&mut file, &mut bytes[offset..])
            .map_err(|e| e.to_string())?;
        require(n > 0, "file ended before stat size")?;
        offset += n;
    }
    fs.close(file).map_err(|e| e.to_string())?;
    Ok(bytes)
}
fn write_file(fs: &mut Fs, path: &str, bytes: &[u8]) -> Result<()> {
    let mut file = fs
        .open(path, OpenFlags::WRITE)
        .map_err(|e| format!("open {path}: {e}"))?;
    let mut offset = 0;
    while offset < bytes.len() {
        let n = fs
            .write(&mut file, &bytes[offset..])
            .map_err(|e| e.to_string())?;
        require(n > 0, "file write made no progress")?;
        offset += n;
    }
    fs.sync(&mut file).map_err(|e| e.to_string())?;
    fs.close(file).map_err(|e| e.to_string())
}
fn walk(fs: &mut Fs, path: &str, tree: &mut Tree) -> Result<()> {
    for entry in fs.read_dir(path).map_err(|e| format!("list {path}: {e}"))? {
        let child = format!("{}/{}", path.trim_end_matches('/'), entry.name);
        match entry.meta.kind {
            Kind::Dir => {
                tree.insert(child.clone(), Entry::Directory);
                walk(fs, &child, tree)?;
            }
            Kind::File => {
                tree.insert(child.clone(), Entry::File(read_file(fs, &child)?));
            }
        }
    }
    Ok(())
}
fn snapshot(bytes: &[u8]) -> Result<Tree> {
    require(
        !bytes.is_empty() && bytes.len() % 4096 == 0,
        "image must contain complete 4096-byte sectors",
    )?;
    let mut mounted = Fs::mount(
        Box::new(Shared::image(bytes.to_vec())),
        Geometry::flash((bytes.len() / 4096) as u32),
        true,
    )
    .map_err(|e| format!("read-only mount: {e}"))?;
    let mut tree = Tree::new();
    walk(&mut mounted, "/", &mut tree)?;
    Ok(tree)
}
fn inventory(tree: &Tree) -> Value {
    Value::Object(
        tree.iter()
            .map(|(path, entry)| {
                let value = match entry {
                    Entry::Directory => json!({"kind":"directory"}),
                    Entry::File(bytes) => {
                        json!({"kind":"file","bytes":bytes.len(),"sha256":sha(bytes)})
                    }
                };
                (path.clone(), value)
            })
            .collect(),
    )
}

struct Package {
    script: Vec<u8>,
    manifest: Vec<u8>,
    icon: Vec<u8>,
}
fn validate_package(package: &Package) -> Result<()> {
    let expected_script = std::str::from_utf8(UPSTREAM_SCRIPT)
        .unwrap()
        .replace(
            "    local columns = math.max(2, math.floor((width - 16 + GRID_GAP) / 96))",
            "    local columns = 3",
        )
        .replace(
            "    screen:load()\n    show_grid()",
            "    show_grid()\n    screen:load()",
        );
    require(
        package.script == expected_script.as_bytes(),
        "maintained script differs beyond three columns and first-frame ordering",
    )?;
    let mut expected: Value =
        serde_json::from_slice(UPSTREAM_MANIFEST).map_err(|e| e.to_string())?;
    expected["icon"] = json!("assets/icon.jpg");
    let actual: Value = serde_json::from_slice(&package.manifest).map_err(|e| e.to_string())?;
    require(
        actual == expected,
        "manifest changes must be limited to the icon path",
    )?;
    let icon = mosaico_lua::jpeg::decode(&package.icon).map_err(|e| format!("icon JPEG: {e:?}"))?;
    require(
        icon.width == 96 && icon.height == 96,
        "launcher icon must be 96x96",
    )
}
fn patch(input: &[u8], package: &Package) -> Result<(Vec<u8>, Value)> {
    validate_package(package)?;
    let before = snapshot(input)?;
    require(
        before.get(SCRIPT) == Some(&Entry::File(UPSTREAM_SCRIPT.to_vec())),
        "source Album script differs from frozen upstream",
    )?;
    require(
        before.get(MANIFEST) == Some(&Entry::File(UPSTREAM_MANIFEST.to_vec())),
        "source Album manifest differs from frozen upstream",
    )?;
    require(
        !before.contains_key(ICON),
        "target icon already exists; review an explicit upgrade plan",
    )?;
    if let Some(entry) = before.get(ASSETS) {
        require(*entry == Entry::Directory, "assets path is not a directory")?;
    }
    let dev = Shared::image(input.to_vec());
    {
        let mut fs = Fs::mount(
            Box::new(dev.clone()),
            Geometry::flash((input.len() / 4096) as u32),
            false,
        )
        .map_err(|e| format!("mount existing RAM image: {e}"))?;
        if !before.contains_key(ASSETS) {
            fs.mkdir(ASSETS).map_err(|e| e.to_string())?;
        }
        write_file(&mut fs, SCRIPT, &package.script)?;
        write_file(&mut fs, MANIFEST, &package.manifest)?;
        write_file(&mut fs, ICON, &package.icon)?;
    }
    let output = dev.bytes();
    let after = snapshot(&output)?;
    let mut expected = before.clone();
    expected.insert(SCRIPT.into(), Entry::File(package.script.clone()));
    expected.insert(MANIFEST.into(), Entry::File(package.manifest.clone()));
    expected.insert(ASSETS.into(), Entry::Directory);
    expected.insert(ICON.into(), Entry::File(package.icon.clone()));
    require(
        after == expected,
        "candidate tree differs beyond the three allowed Album files and assets directory",
    )?;
    let mut vfs = Vfs::new();
    vfs.mount(
        "system",
        Fs::mount(
            Box::new(Shared::image(output.clone())),
            Geometry::flash((input.len() / 4096) as u32),
            true,
        )
        .map_err(|e| e.to_string())?,
    );
    let mut registry_log = Vec::new();
    let registry = Registry::scan(&mut vfs, &mut IconCache::new(), &mut |line| {
        registry_log.push(line.to_owned())
    });
    let album = registry
        .find("album_app")
        .map(|i| &registry.apps[i])
        .ok_or("candidate Album missing from registry")?;
    require(
        album.origin == Origin::System && album.runnable(),
        "candidate Album must remain runnable Origin::System",
    )?;
    let icon = album
        .icon
        .ok_or("candidate icon did not decode in the registry")?;
    require(
        icon.width == 96 && icon.height == 96,
        "registry icon dimensions changed",
    )?;
    let report = json!({"status":"verified-existing-system-overlay", "input_bytes":input.len(), "input_sha256":sha(input),
        "output_bytes":output.len(), "output_sha256":sha(&output), "changed_files":[SCRIPT,MANIFEST,ICON],
        "added_directory":if before.contains_key(ASSETS) { Value::Null } else { json!(ASSETS) },
        "unchanged_entries": before.keys().filter(|p| p.as_str()!=SCRIPT && p.as_str()!=MANIFEST).count(),
        "album":{"origin":"System","runnable":true,"title":album.title,"icon_width":icon.width,"icon_height":icon.height},
        "registry_log":registry_log,"before":inventory(&before),"after":inventory(&after)});
    Ok((output, report))
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err(
            "usage: album_system_image <system-before.bin> <maintained-album-dir> <new-output-dir>"
                .into(),
        );
    }
    let input = fs::read(&args[1])?;
    let root = Path::new(&args[2]);
    let package = Package {
        script: fs::read(root.join("scripts/album_app.lua"))?,
        manifest: fs::read(root.join("launcher.json"))?,
        icon: fs::read(root.join("assets/icon.jpg"))?,
    };
    let (image, report) = patch(&input, &package)?;
    let out = Path::new(&args[3]);
    fs::create_dir(out)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join("system-candidate.bin"))?
        .write_all(&image)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join("file-tree-report.json"))?
        .write_all(serde_json::to_string_pretty(&report)?.as_bytes())?;
    println!(
        "{}",
        json!({"status":report["status"],"output":out.display().to_string(),"unchanged_entries":report["unchanged_entries"],"album":report["album"]})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package() -> Package {
        Package {
            script: include_bytes!("../../../apps/album_app/scripts/album_app.lua").to_vec(),
            manifest: include_bytes!("../../../apps/album_app/launcher.json").to_vec(),
            icon: include_bytes!("../../../apps/album_app/assets/icon.jpg").to_vec(),
        }
    }
    fn source(script: &[u8]) -> Vec<u8> {
        let dev = Shared::image(vec![0xff; 4096 * 128]);
        let (mut fs, _) = Fs::mount_or_format(Box::new(dev.clone()), Geometry::flash(128)).unwrap();
        for dir in [
            "/apps",
            "/apps/album_app",
            "/apps/album_app/scripts",
            "/fonts",
            "/unrelated",
        ] {
            fs.mkdir(dir).unwrap();
        }
        write_file(&mut fs, SCRIPT, script).unwrap();
        write_file(&mut fs, MANIFEST, UPSTREAM_MANIFEST).unwrap();
        write_file(&mut fs, "/fonts/keep.bin", &[0, 1, 255, 42]).unwrap();
        write_file(&mut fs, "/unrelated/note.txt", b"keep exact bytes").unwrap();
        drop(fs);
        dev.bytes()
    }
    #[test]
    fn preserves_all_unrelated_files_and_system_origin() {
        let before = source(UPSTREAM_SCRIPT);
        let (after, report) = patch(&before, &package()).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(report["album"]["origin"], "System");
        assert_eq!(
            snapshot(&before).unwrap()["/fonts/keep.bin"],
            snapshot(&after).unwrap()["/fonts/keep.bin"]
        );
        assert!(
            patch(&after, &package()).is_err(),
            "do not overwrite a changed Album without a new plan"
        );
    }
    #[test]
    fn rejects_modified_source_and_invalid_packages() {
        assert!(patch(&source(b"already customized"), &package()).is_err());
        let before = source(UPSTREAM_SCRIPT);
        let mut p = package();
        p.script.extend_from_slice(b"\n-- unexpected behavior");
        assert!(patch(&before, &p).is_err());
        let mut p = package();
        p.manifest = b"{}".to_vec();
        assert!(patch(&before, &p).is_err());
        let mut p = package();
        p.icon = b"bad JPEG".to_vec();
        assert!(patch(&before, &p).is_err());
        assert!(patch(&before[..before.len() - 1], &package()).is_err());
        assert!(
            patch(&vec![0xff; 4096 * 128], &package()).is_err(),
            "never format invalid input"
        );
    }
}
