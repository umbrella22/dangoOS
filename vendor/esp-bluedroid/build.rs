use std::{fs, path::Path};
fn include_dirs(dir: &Path, build: &mut cc::Build) {
    let mut paths: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path.file_name().unwrap() == "include"
                || path.parent().unwrap().file_name().unwrap() == "include"
            {
                build.include(&path);
            }
            include_dirs(&path, build);
        }
    }
}
fn main() {
    println!("cargo:rerun-if-changed=upstream");
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=sources.txt");
    println!("cargo:rerun-if-changed=src/classic.c");
    println!("cargo:rerun-if-changed=src/adapter.c");
    println!("cargo:rerun-if-changed=src/crypto.c");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() != "none" {
        return;
    }
    let mut b = cc::Build::new();
    // #region debug-point Q-R-S:native-work-item
    if std::env::var_os("CARGO_FEATURE_WORK_DIAGNOSTICS").is_some() {
        b.define("MOSAICO_BT_WORK_DIAGNOSTICS", None);
    }
    // #endregion
    b.include("shim");
    include_dirs(Path::new("upstream"), &mut b);
    b.flag("-std=gnu11")
        .flag("-ffunction-sections")
        .flag("-fdata-sections")
        .flag("-include")
        .flag("shim/esp_compiler.h")
        .flag("-Werror=implicit-function-declaration")
        .flag("-Wno-address-of-packed-member")
        .warnings(false);
    for line in fs::read_to_string("sources.txt").unwrap().lines() {
        if !line.is_empty() {
            b.file(line);
        }
    }
    b.file("src/classic.c")
        .file("src/adapter.c")
        .file("src/crypto.c")
        .compile("mosaico_bluedroid");
}
