//! Compiles the vendored littlefs 2.11.3 plus the C glue.
//!
//! Host builds (tests) use the host C compiler. The firmware build also
//! compiles `csrc/mosaico_fs_posix.c`, the newlib syscalls (`_open_r`,
//! `stat`, `opendir`, ...) that route C file access to [`vfs`](src/vfs.rs).

use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lfs = root.join("vendor/littlefs-2.11.3");
    let dhara = root.join("vendor/dhara");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let mut build = cc::Build::new();
    build
        .include(&lfs)
        .include(dhara.join("include"))
        // No logging or asserts: the firmware has no stdio console, and a
        // failed assert must not abort. Errors still come back as codes.
        .define("LFS_NO_DEBUG", None)
        .define("LFS_NO_WARN", None)
        .define("LFS_NO_ERROR", None)
        .define("LFS_NO_ASSERT", None)
        .flag_if_supported("-std=gnu99")
        .opt_level(2)
        .warnings(false);
    if target_os == "none" {
        build.flag("-ffunction-sections").flag("-fdata-sections");
    }
    build.file(lfs.join("lfs.c"));
    build.file(lfs.join("lfs_util.c"));
    build.file(root.join("csrc/mosaico_fs_glue.c"));
    build.file(dhara.join("dhara/map.c"));
    build.file(dhara.join("dhara/journal.c"));
    build.file(root.join("csrc/mosaico_nand.c"));
    if target_os == "none" {
        build.file(root.join("csrc/mosaico_fs_posix.c"));
    }
    build.compile("mosaico_fs");

    if target_os == "none" {
        // newlib from the riscv32-esp-elf toolchain, matching -march/-mabi.
        // Linked here, after this crate's archive, so the syscalls below win
        // over newlib's stubs (lua-engine depends on this crate).
        let cc = build.get_compiler();
        let mut args: Vec<String> = Vec::new();
        for arg in cc.args() {
            let a = arg.to_string_lossy();
            if a.starts_with("-march") || a.starts_with("-mabi") {
                args.push(a.into_owned());
            }
        }
        for lib in ["libc.a", "libm.a", "libgcc.a"] {
            let out = std::process::Command::new(cc.path())
                .args(&args)
                .arg(format!("-print-file-name={lib}"))
                .output()
                .expect("run the C compiler to locate newlib");
            let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if let Some(dir) = path.parent().filter(|_| path.is_absolute()) {
                println!("cargo:rustc-link-search=native={}", dir.display());
            } else {
                panic!("{lib} not found by {}", cc.path().display());
            }
        }
        println!("cargo:rustc-link-lib=static:-bundle=m");
        println!("cargo:rustc-link-lib=static:-bundle=c");
        println!("cargo:rustc-link-lib=static:-bundle=gcc");
    }

    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed=vendor");
}
