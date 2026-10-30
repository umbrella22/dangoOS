//! Keep the S31 slot budget in this repository, separate from Cargo caches.
fn main() {
    println!("cargo:rerun-if-env-changed=DANGO_UPDATE_TEST_CA");
    println!("cargo:rerun-if-env-changed=DANGO_UPDATE_MANIFEST_URL");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ca = std::env::var_os("DANGO_UPDATE_TEST_CA")
        .map(|path| {
            println!(
                "cargo:rerun-if-changed={}",
                std::path::Path::new(&path).display()
            );
            std::fs::read(path).expect("update test CA")
        })
        .unwrap_or_default();
    std::fs::write(out.join("update-test-ca.der"), ca).unwrap();
    let build_unix = std::env::var("SOURCE_DATE_EPOCH")
        .map(|value| value.parse::<u64>().expect("SOURCE_DATE_EPOCH seconds"))
        .unwrap_or_else(|_| std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs());
    println!("cargo:rustc-env=DANGO_BUILD_UNIX={build_unix}");
    println!("cargo:rerun-if-changed=ld");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        println!("cargo:rustc-link-search={}", root.join("ld").display());
    }
}
