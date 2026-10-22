//! Compiles the vendored Lua 5.5.1 core plus the C glue.
//!
//! Host builds (tests) use the host C compiler; the firmware build uses the
//! compiler Cargo is told about through `CC_riscv32imafc_unknown_none_elf`
//! (see `.cargo/config.toml`). The firmware links newlib's libc/libm from that
//! toolchain for snprintf/strtod/math; `mosaico-s31` supplies malloc and the
//! syscall stubs.

use std::path::PathBuf;

const LUA_DIR: &str = "vendor/lua-5.5.1";

/// Lua core and the libraries we open. Not built: lua.c/luac.c (programs),
/// liolib.c/loslib.c (file system, process, clock; replaced by host modules),
/// linit.c (the glue opens the libraries itself).
const LUA_SOURCES: &[&str] = &[
    "lapi.c",
    "lcode.c",
    "lctype.c",
    "ldebug.c",
    "ldo.c",
    "ldump.c",
    "lfunc.c",
    "lgc.c",
    "llex.c",
    "lmem.c",
    "lobject.c",
    "lopcodes.c",
    "lparser.c",
    "lstate.c",
    "lstring.c",
    "ltable.c",
    "ltm.c",
    "lundump.c",
    "lvm.c",
    "lzio.c",
    "lauxlib.c",
    "lbaselib.c",
    "lcorolib.c",
    "ldblib.c",
    "lmathlib.c",
    "lstrlib.c",
    "ltablib.c",
    "lutf8lib.c",
    "loadlib.c",
];

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lua = root.join(LUA_DIR);
    let user_h = root.join("csrc/mosaico_lua_user.h");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let mut build = cc::Build::new();
    build
        .include(&lua)
        .flag(format!("-include{}", user_h.display()))
        .flag_if_supported("-std=gnu99")
        .flag_if_supported("-Wno-unused-parameter")
        .opt_level(2)
        .warnings(false);
    if target_os == "none" {
        build
            .flag("-ffunction-sections")
            .flag("-fdata-sections")
            // Lua needs a hosted-ish libc (newlib), not freestanding builtins only.
            .flag("-fno-builtin-printf");
    }
    for file in LUA_SOURCES {
        build.file(lua.join(file));
    }
    build.file(root.join("csrc/mosaico_lua_glue.c"));
    build.compile("mosaico_lua");

    if target_os == "none" {
        // newlib from the riscv32-esp-elf toolchain, matching -march/-mabi.
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
    println!("cargo:rerun-if-changed={LUA_DIR}");
}
