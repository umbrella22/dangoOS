fn main() {
    println!("cargo:rerun-if-changed=ld");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        println!("cargo:rustc-link-search={}", root.join("ld").display());
    }
}
