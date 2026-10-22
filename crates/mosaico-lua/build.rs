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
/// liolib.c (file system; arrives with storage), linit.c (the glue opens the
/// libraries itself). loslib.c is built through csrc/mosaico_loslib.c.
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
    "liolib.c",
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
    build.file(root.join("csrc/mosaico_loslib.c"));
    build.compile("mosaico_lua");

    // esp-claw modules (Apache-2.0) and cJSON (MIT), built against the
    // ESP-IDF shims in csrc/esp_shim. Separate library: no Lua user header
    // tweaks are needed and the module sources stay unchanged.
    let claw = root.join("vendor/esp-claw");
    let mut modules = cc::Build::new();
    modules
        .include(&lua)
        .include(root.join("csrc"))
        .include(root.join("csrc/esp_shim"))
        .include(claw.join("lua_module_display"))
        .include(claw.join("lua_module_json"))
        .include(claw.join("lua_module_storage"))
        .include(claw.join("esp_painter"))
        .include(root.join("vendor/cjson"))
        .flag(format!("-include{}", user_h.display()))
        .flag_if_supported("-std=gnu11")
        .opt_level(2)
        .warnings(false);
    if target_os == "none" {
        modules
            .flag("-ffunction-sections")
            .flag("-fdata-sections")
            .flag("-fno-builtin-printf");
    }
    for file in [
        "lua_module_display/display_bitmap.c",
        "lua_module_display/display_color.c",
        "lua_module_display/display_core.c",
        "lua_module_display/display_dirty.c",
        "lua_module_display/display_raster.c",
        "lua_module_display/display_text.c",
        "lua_module_display/lua_module_display.c",
        "lua_module_json/lua_module_json.c",
        "lua_module_storage/lua_module_storage.c",
        "esp_painter/basic_font_12.c",
        "esp_painter/basic_font_16.c",
        "esp_painter/basic_font_20.c",
        "esp_painter/basic_font_24.c",
        "esp_painter/basic_font_28.c",
        "esp_painter/basic_font_32.c",
        "esp_painter/basic_font_36.c",
        "esp_painter/basic_font_40.c",
        "esp_painter/basic_font_44.c",
        "esp_painter/basic_font_48.c",
    ] {
        modules.file(claw.join(file));
    }
    modules.file(root.join("vendor/cjson/cJSON.c"));
    modules.file(root.join("csrc/mosaico_platform.c"));
    // arg_schema.lua is compiled into the image as a C string.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let src = std::fs::read(claw.join("lua_module_system/arg_schema.lua")).unwrap();
    let mut c = String::from("#include <stddef.h>\nconst char mosaico_arg_schema_lua[] = {");
    for b in &src {
        c.push_str(&format!("{b},"));
    }
    c.push_str(&format!(
        "0}};\nconst size_t mosaico_arg_schema_lua_len = {};\n",
        src.len()
    ));
    let gen = out.join("mosaico_embedded.c");
    std::fs::write(&gen, c).unwrap();
    modules.file(gen);
    // The ESP32-S31 ROM has its own TJpgDec, and the esp-hal ROM linker
    // script defines `jd_prepare`/`jd_decomp` at its addresses. Its `JDEC`
    // layout differs from ours, so rename our copy rather than call the ROM.
    modules
        .define("jd_prepare", "mosaico_jd_prepare")
        .define("jd_decomp", "mosaico_jd_decomp")
        .include(root.join("vendor/tjpgd-r0.03"))
        .file(root.join("vendor/tjpgd-r0.03/tjpgd.c"))
        .file(root.join("csrc/mosaico_jpeg.c"));
    modules.compile("mosaico_lua_modules");

    // newlib (libc/libm/libgcc) is linked by mosaico-fs, after its syscalls,
    // so the linker resolves `_open_r` & co. there rather than to newlib's
    // own stubs.
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed={LUA_DIR}");
}
