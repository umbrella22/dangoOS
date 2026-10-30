use app_pack::{
    image::{self, Entry},
    Package,
};
use std::{
    fs,
    io::Write,
    path::{Component, Path},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn output(
    dir: &Path,
    name: &str,
    bytes: &[u8],
    report: &serde_json::Value,
    packages: &[Package],
) -> Result<()> {
    fs::create_dir(dir)?;
    write_new(&dir.join(name), bytes)?;
    for package in packages {
        let docs = dir.join("application-docs").join(&package.id);
        for (path, entry) in &package.documentation {
            let Entry::File(bytes) = entry else {
                return Err("application documentation must be a file".into());
            };
            fs::create_dir_all(&docs)?;
            write_new(&docs.join(path), bytes)?;
        }
    }
    write_new(
        &dir.join("file-tree-report.json"),
        &serde_json::to_vec_pretty(report)?,
    )?;
    println!(
        "{}",
        serde_json::json!({"status":report["status"],"output":dir.join(name),"sha256":report["output_sha256"]})
    );
    Ok(())
}

fn extract(input: &Path, out: &Path) -> Result<()> {
    let bytes = fs::read(input)?;
    let tree = image::snapshot(&bytes)?;
    if tree.contains_key("/extraction-receipt.json") {
        return Err("image contains reserved receipt path".into());
    }
    for path in tree.keys() {
        let relative = path.strip_prefix('/').ok_or("non-absolute image path")?;
        if !Path::new(relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return Err("unsafe image path".into());
        }
    }
    fs::create_dir(out)?;
    for (path, entry) in &tree {
        let target = out.join(path.trim_start_matches('/'));
        match entry {
            Entry::Directory => fs::create_dir_all(target)?,
            Entry::File(bytes) => {
                fs::create_dir_all(target.parent().unwrap())?;
                write_new(&target, bytes)?;
                if fs::read(&target)? != *bytes {
                    return Err("extracted file verification failed".into());
                }
            }
        }
    }
    let receipt = serde_json::json!({
        "input_sha256": image::sha(&bytes),
        "files": image::inventory(&tree),
    });
    write_new(
        &out.join("extraction-receipt.json"),
        &serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("extracted and verified {} entries", tree.len());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("skins") if args.len() == 3 => {
            println!("{}", serde_json::json!({
                "status": "valid-skin-packages",
                "skins": app_pack::skins::check(Path::new(&args[2]))?,
            }));
        }
        Some("extract") if args.len() == 4 => {
            extract(Path::new(&args[2]), Path::new(&args[3]))?;
        }
        Some("system") if args.len() == 6 && args[4] == "--size" => {
            let packages = app_pack::discover(Path::new(&args[2]))?;
            let image_bytes = args[5].parse::<usize>()?;
            let (bytes, report) = app_pack::build_system(image_bytes, &packages)?;
            output(Path::new(&args[3]), "system.bin", &bytes, &report, &packages)?;
        }
        Some("check") if args.len() == 3 => {
            let packages = app_pack::discover(Path::new(&args[2]))?;
            if packages.is_empty() {
                return Err("no applications found".into());
            }
            println!("{}", serde_json::json!({
                "status": "valid-application-packages",
                "ids": packages.iter().map(|p| &p.id).collect::<Vec<_>>(),
            }));
        }
        _ => return Err("usage: app-pack extract <image> <new-output-dir> | system <apps-root> <new-output-dir> --size <partition-bytes> | check <apps-root> | skins <skin-root>".into()),
    }
    Ok(())
}
