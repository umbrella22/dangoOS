fn main() {
    let mut build = cc::Build::new();
    build
        .include("vendor/tjpgd-r0.03")
        .include("vendor/stb")
        .define("jd_prepare", "mosaico_jd_prepare")
        .define("jd_decomp", "mosaico_jd_decomp")
        .file("vendor/tjpgd-r0.03/tjpgd.c")
        .file("csrc/mosaico_jpeg.c")
        .file("csrc/mosaico_jpeg_encode.c")
        .flag_if_supported("-ffunction-sections")
        .flag_if_supported("-fdata-sections")
        .warnings(false)
        .opt_level(2)
        .compile("dango_app_host");
    if std::env::var_os("CARGO_FEATURE_GRAPHICS").is_some() {
        graphics();
    }
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed=vendor");
}

fn graphics() {
    use std::{collections::BTreeSet, fs, path::PathBuf};
    let mut names = BTreeSet::new();
    for entry in fs::read_dir("vendor/display").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "h" || e == "c") {
            let source = fs::read_to_string(path).unwrap();
            for word in source.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                if word.starts_with("display_") || word.starts_with("esp_painter_basic_font_") {
                    names.insert(word.to_owned());
                }
            }
        }
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let mut prefix = String::new();
    for name in names {
        prefix.push_str(&format!("#define {name} dango_{name}\n"));
    }
    // LodePNG also coexists with both LVGL and the Lua image module.
    for word in fs::read_to_string("vendor/lodepng/lodepng.h")
        .unwrap()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
    {
        if word.starts_with("lodepng_") {
            prefix.push_str(&format!("#define {word} dango_{word}\n"));
        }
    }
    fs::write(out.join("graphics-prefix.h"), prefix).unwrap();
    let mut build = cc::Build::new();
    build
        .include("vendor/display")
        .include("vendor/lodepng")
        .include("csrc")
        .flag(format!(
            "-include{}",
            out.join("graphics-prefix.h").display()
        ))
        .define("LODEPNG_NO_COMPILE_CPP", None)
        .define("LODEPNG_NO_COMPILE_ENCODER", None)
        .define("LODEPNG_NO_COMPILE_DISK", None)
        .file("vendor/lodepng/lodepng.c")
        .file("csrc/graphics.c");
    for name in [
        "display_color",
        "display_raster",
        "display_dirty",
        "display_bitmap",
    ] {
        build.file(format!("vendor/display/{name}.c"));
    }
    for size in [12, 16, 20, 24, 28, 32, 36, 40, 44, 48] {
        build.file(format!("vendor/display/basic_font_{size}.c"));
    }
    build
        .flag_if_supported("-ffunction-sections")
        .flag_if_supported("-fdata-sections")
        .flag_if_supported("-std=gnu11")
        .warnings(false)
        .opt_level(2)
        .compile("dango_graphics");
}
