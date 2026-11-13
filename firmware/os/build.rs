//! Keep the S31 slot budget in this repository, separate from Cargo caches.
#[path = "build/fonts.rs"]
mod fonts;
fn main() {
    println!("cargo:rerun-if-changed=build/fonts.rs");
    fonts::generate();
    println!("cargo:rerun-if-env-changed=DANGO_EMBEDSCOPE_BUILD_ID");
    if std::env::var_os("CARGO_FEATURE_EMBEDSCOPE").is_some() {
        scope_build_id();
    }
    println!("cargo:rerun-if-env-changed=DANGO_UPDATE_MANIFEST_URL");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let build_unix = std::env::var("SOURCE_DATE_EPOCH")
        .map(|value| value.parse::<u64>().expect("SOURCE_DATE_EPOCH seconds"))
        .unwrap_or_else(|_| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });
    println!("cargo:rustc-env=DANGO_BUILD_UNIX={build_unix}");
    println!("cargo:rerun-if-changed=ld");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        println!("cargo:rustc-link-search={}", root.join("ld").display());
    }
}

/// Fingerprint local integration sources and locked registry versions/checksums.
fn scope_build_id() {
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    fn files(path: &Path, output: &mut Vec<PathBuf>) {
        if path.is_dir() {
            let mut children: Vec<_> = std::fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            children.sort();
            for child in children {
                files(&child, output);
            }
        } else if path
            .extension()
            .is_some_and(|e| e == "rs" || e == "toml" || e == "lock")
        {
            output.push(path.to_owned());
        }
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.parent().unwrap().parent().unwrap();
    let mut paths = Vec::new();
    for path in [
        manifest.join("src"),
        manifest.join("Cargo.toml"),
        manifest.join("build.rs"),
        workspace.join("Cargo.toml"),
        workspace.join("Cargo.lock"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
        files(&path, &mut paths);
    }
    let mut digest = Sha256::new();
    for path in paths {
        digest.update(
            path.strip_prefix(workspace)
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        digest.update([0]);
        digest.update(std::fs::read(path).unwrap());
        digest.update([0]);
    }
    let hash = format!("{:x}", digest.finalize());
    let default = format!(
        "dango-{}+escope.{}",
        std::env::var("CARGO_PKG_VERSION").unwrap(),
        &hash[..20]
    );
    let id = std::env::var("DANGO_EMBEDSCOPE_BUILD_ID").unwrap_or(default);
    assert!(
        !id.is_empty() && id.len() <= 64,
        "EmbedScope build_id must have 1..64 UTF-8 bytes"
    );
    println!("cargo:rustc-env=DANGO_EMBEDSCOPE_BUILD_ID={id}");
}
