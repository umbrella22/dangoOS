use super::*;

fn apps() -> Vec<Package> {
    discover(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps")).unwrap()
}

#[test]
fn fresh_system_contains_exactly_repository_apps_and_rebuilds_identically() {
    let packages = apps();
    assert_eq!(
        packages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["2048", "dino", "flappy", "photo", "snake"]
    );
    for p in &packages {
        assert!(p.documentation.contains_key("LICENSE"));
        for name in ["LICENSE", "README.md", "SOURCE.json"] {
            assert!(!p.files.contains_key(name));
        }
        let Entry::File(bytes) = &p.files["launcher.json"] else {
            panic!()
        };
        let manifest: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(manifest["entry"], "scripts/main.lua");
    }
    let (image, report) = build_system(4096 * 750, &packages).unwrap();
    assert_eq!(image.len(), 4096 * 750);
    assert_eq!(report["applications"].as_array().unwrap().len(), 5);
    let tree = snapshot(&image).unwrap();
    assert!(tree
        .keys()
        .all(|path| path == "/apps" || path.starts_with("/apps/")));
    assert!(tree.contains_key("/apps/snake/scripts/main.lua"));
    assert!(tree.contains_key("/apps/2048/scripts/main.lua"));
    let (again, _) = build_system(image.len(), &packages).unwrap();
    assert_eq!(image, again, "rebuild must be reproducible");

    let (_, mut vfs) = mount(&image, "system", false).unwrap();
    let data = Fs::mount_or_format(
        Box::new(::fs::RamDevice::new(4096, 64)),
        Geometry::flash(64),
    )
    .unwrap()
    .0;
    vfs.mount("data", data);
    vfs.create_dir_all("/data/apps/snake/scripts").unwrap();
    vfs.write_file("/data/apps/snake/launcher.json", br#"{"schema_version":1,"id":"snake","entry":"scripts/main.lua","display_name":"Updated Snake"}"#).unwrap();
    vfs.write_file("/data/apps/snake/scripts/main.lua", b"return 42")
        .unwrap();
    let registry = Registry::scan(&mut vfs, &mut IconCache::new(), &mut |_| {});
    assert_eq!(registry.apps.len(), 5);
    let snake = &registry.apps[registry.find("snake").unwrap()];
    assert_eq!(snake.origin, Origin::Data);
    assert_eq!(snake.app.entry, "/data/apps/snake/scripts/main.lua");
    let id = os::hub::uninstall::Identity::new("snake", Origin::Data).unwrap();
    os::hub::uninstall::execute(&mut vfs, os::hub::uninstall::Operation::Remove(id)).unwrap();
    let registry = Registry::scan(&mut vfs, &mut IconCache::new(), &mut |_| {});
    assert_eq!(
        registry.apps[registry.find("snake").unwrap()].origin,
        Origin::System
    );
}

#[test]
fn invalid_packages_duplicate_ids_and_wrong_partition_sizes_are_rejected() {
    let size = 4096 * 750;
    let mut packages = apps();
    packages[0].files.remove("scripts/main.lua");
    assert!(build_system(size, &packages).is_err());
    let mut packages = apps();
    packages[0]
        .files
        .insert("../escape".into(), Entry::File(vec![0]));
    assert!(build_system(size, &packages).is_err());
    let mut packages = apps();
    packages[0]
        .files
        .insert("assets/icon.jpg".into(), Entry::File(b"bad JPEG".to_vec()));
    assert!(build_system(size, &packages).is_err());
    let mut packages = apps();
    packages.push(apps().remove(0));
    assert!(build_system(size, &packages)
        .unwrap_err()
        .contains("duplicate"));
    assert!(build_system(0, &apps()).is_err());
    assert!(build_system(size - 1, &apps()).is_err());
    assert!(build_system(4096 * 8, &apps())
        .unwrap_err()
        .contains("space"));
}

#[test]
fn development_examples_and_device_probes_are_valid_but_not_factory_apps() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut ids = BTreeSet::new();
    for directory in ["examples/apps", "tests/device/apps"] {
        for app in host_fs::read_dir(root.join(directory)).unwrap() {
            let package = Package::read(&app.unwrap().path()).unwrap();
            assert!(ids.insert(package.id));
        }
    }
    assert_eq!(ids.len(), 7);
    assert!(apps().iter().all(|p| !ids.contains(&p.id)));
}
