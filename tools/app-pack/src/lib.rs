//! Build fresh LittleFS system images from repository application packages.
pub mod image;

use ::fs::{Fs, Geometry, Vfs};
use image::{inventory, require, sha, snapshot, Entry, Result, Shared, Tree};
use os::hub::registry::{self, IconCache, Origin, Registry};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs as host_fs, path::Path};

pub struct Package {
    pub id: String,
    pub files: Tree,
    /// Source documentation accompanies the image on the host, outside flash.
    pub documentation: Tree,
}
fn collect(root: &Path, relative: &Path, files: &mut Tree) -> Result<()> {
    for e in host_fs::read_dir(root.join(relative)).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let rel = relative.join(e.file_name());
        let name = rel.to_str().ok_or("non-UTF-8 application path")?.to_owned();
        require(
            registry::relative_path_is_valid(&name),
            "invalid application resource path",
        )?;
        let kind = e.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            files.insert(name, Entry::Directory);
            collect(root, &rel, files)?;
        } else if kind.is_file() {
            files.insert(
                name,
                Entry::File(host_fs::read(e.path()).map_err(|e| e.to_string())?),
            );
        } else {
            return Err(format!(
                "unsupported application resource: {}",
                e.path().display()
            ));
        }
    }
    Ok(())
}
impl Package {
    pub fn read(root: &Path) -> Result<Self> {
        let id = root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("missing application directory name")?
            .to_owned();
        let mut files = Tree::new();
        collect(root, Path::new(""), &mut files)?;
        let mut documentation = Tree::new();
        for name in ["README.md", "LICENSE", "SOURCE.json"] {
            if let Some(entry) = files.remove(name) {
                require(
                    matches!(entry, Entry::File(_)),
                    "application documentation must be a file",
                )?;
                documentation.insert(name.into(), entry);
            }
        }
        let package = Self {
            id,
            files,
            documentation,
        };
        package.validate()?;
        Ok(package)
    }
    pub fn validate(&self) -> Result<()> {
        for path in self.files.keys() {
            require(
                registry::relative_path_is_valid(path),
                "invalid application resource path",
            )?;
        }
        let Some(Entry::File(bytes)) = self.files.get("launcher.json") else {
            return Err("launcher.json missing".into());
        };
        require(
            bytes.len() <= registry::MANIFEST_MAX_BYTES as usize,
            "launcher.json exceeds registry limit",
        )?;
        let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let manifest = registry::parse_manifest(text, &self.id)?;
        require(
            matches!(self.files.get(&manifest.entry), Some(Entry::File(_))),
            "application entry missing",
        )?;
        if let Some(icon) = manifest.icon {
            let Some(Entry::File(bytes)) = self.files.get(&icon) else {
                return Err(format!("icon missing: {icon}"));
            };
            lua_engine::jpeg::decode(bytes).map_err(|e| format!("{} icon: {e:?}", self.id))?;
        }
        Ok(())
    }
}
fn mount(input: &[u8], name: &str, writable: bool) -> Result<(Shared, Vfs)> {
    let disk = Shared::image(input.to_vec());
    let fs = Fs::mount(
        Box::new(disk.clone()),
        Geometry::flash((input.len() / 4096) as u32),
        !writable,
    )
    .map_err(|e| format!("mount {name}: {e}"))?;
    let mut vfs = Vfs::new();
    vfs.mount(name, fs);
    Ok((disk, vfs))
}
fn system_error(vfs: &mut Vfs, operation: &str, path: &str, error: ::fs::Errno) -> String {
    match vfs.space("/system") {
        Ok(space) => format!(
            "{operation} {path}: {error} ({} of {} blocks free, {} bytes per block)",
            space.free, space.blocks, space.block_size
        ),
        Err(_) => format!("{operation} {path}: {error}"),
    }
}
/// Create a new system filesystem sized to the target partition. No existing
/// device image or user data participates in the build.
pub fn build_system(image_bytes: usize, packages: &[Package]) -> Result<(Vec<u8>, Value)> {
    require(
        image_bytes >= 4096 && image_bytes % 4096 == 0,
        "image size must be a positive multiple of 4096 bytes",
    )?;
    let blocks = u32::try_from(image_bytes / 4096).map_err(|e| e.to_string())?;
    let mut ids = BTreeSet::new();
    for package in packages {
        package.validate()?;
        require(ids.insert(package.id.clone()), "duplicate application id")?;
    }
    require(!packages.is_empty(), "no applications selected")?;
    let disk = Shared::image(vec![0xff; image_bytes]);
    let (fs, _) = Fs::mount_or_format(Box::new(disk.clone()), Geometry::flash(blocks))
        .map_err(|e| format!("format new system image: {e}"))?;
    let mut vfs = Vfs::new();
    vfs.mount("system", fs);
    let mut expected = Tree::new();
    vfs.create_dir_all("/system/apps")
        .map_err(|e| system_error(&mut vfs, "mkdir", "/apps", e))?;
    expected.insert("/apps".into(), Entry::Directory);
    for package in packages {
        let current = format!("/apps/{}", package.id);
        vfs.create_dir_all(&format!("/system{current}"))
            .map_err(|e| system_error(&mut vfs, "mkdir", &current, e))?;
        expected.insert(current.clone(), Entry::Directory);
        for (rel, entry) in &package.files {
            let path = format!("{current}/{rel}");
            match entry {
                Entry::Directory => vfs
                    .create_dir_all(&format!("/system{path}"))
                    .map_err(|e| system_error(&mut vfs, "mkdir", &path, e))?,
                Entry::File(bytes) => vfs
                    .write_file(&format!("/system{path}"), bytes)
                    .map_err(|e| system_error(&mut vfs, "write", &path, e))?,
            }
            expected.insert(path, entry.clone());
        }
    }
    drop(vfs);
    let output = disk.bytes();
    let after = snapshot(&output)?;
    require(
        after == expected,
        "output differs from repository application packages",
    )?;
    let (_, mut readonly) = mount(&output, "system", false)?;
    let free_blocks_after = readonly.space("/system").map_err(|e| e.to_string())?.free;
    let mut logs = Vec::new();
    let registry = Registry::scan(&mut readonly, &mut IconCache::new(), &mut |line| {
        logs.push(line.to_owned())
    });
    require(
        registry.apps.len() == packages.len(),
        "registry application count differs from packages",
    )?;
    let mut apps = Vec::new();
    for package in packages {
        let app = registry
            .find(&package.id)
            .map(|i| &registry.apps[i])
            .ok_or_else(|| format!("{} missing from registry: {logs:?}", package.id))?;
        require(
            app.origin == Origin::System && app.runnable(),
            &format!(
                "{} is not a runnable system application: {:?}",
                package.id, app.missing
            ),
        )?;
        if let Some(Entry::File(bytes)) = package.files.get("launcher.json") {
            let manifest: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            if manifest.get("icon").is_some() {
                require(app.icon.is_some(), "registry failed to decode icon")?;
            }
        }
        apps.push(json!({"id":package.id,"title":app.title,"origin":"System","runnable":true}));
    }
    let documentation: serde_json::Map<String, Value> = packages
        .iter()
        .map(|p| (p.id.clone(), inventory(&p.documentation)))
        .collect();
    let report = json!({
        "status":"verified-system-image", "source":"repository-applications",
        "output_sha256":sha(&output), "bytes":output.len(), "applications":apps,
        "files":inventory(&after), "registry_log":logs, "free_blocks":free_blocks_after,
        "application_documentation":documentation
    });
    Ok((output, report))
}
pub fn discover(root: &Path) -> Result<Vec<Package>> {
    let mut packages = Vec::new();
    let mut ids = BTreeSet::new();
    for category in host_fs::read_dir(root).map_err(|e| e.to_string())? {
        let category = category.map_err(|e| e.to_string())?;
        if !category.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        for app in host_fs::read_dir(category.path()).map_err(|e| e.to_string())? {
            let app = app.map_err(|e| e.to_string())?;
            require(
                app.file_type().map_err(|e| e.to_string())?.is_dir(),
                "category must contain application directories",
            )?;
            let package = Package::read(&app.path())?;
            require(
                ids.insert(package.id.clone()),
                "duplicate application id across categories",
            )?;
            packages.push(package);
        }
    }
    packages.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(packages)
}
#[cfg(test)]
mod tests;
