use super::*;

fn apps() -> Vec<Package> {
    discover(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/builtin")).unwrap()
}

#[test]
fn variant_directories_preserve_ids_without_reserving_language_names() {
    let root = std::env::temp_dir().join(format!("dango-variant-ids-{}", std::process::id()));
    assert!(!root.exists());
    for (relative, id) in [("ts", "ts"), ("snake/lua", "snake")] {
        let path = root.join(relative);
        host_fs::create_dir_all(&path).unwrap();
        host_fs::write(
            path.join("launcher.json"),
            format!(r#"{{"schema_version":1,"id":"{id}","entry":"main.lua"}}"#),
        )
        .unwrap();
        host_fs::write(path.join("main.lua"), "return 42").unwrap();
        assert_eq!(Package::read(&path).unwrap().id, id);
    }
    host_fs::write(
        root.join("ts/launcher.json"),
        br#"{"schema_version":1,"id":"other","entry":"main.lua"}"#,
    )
    .unwrap();
    assert!(Package::read(&root.join("ts")).is_err());
    host_fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "lua-engine")]
#[test]
fn fresh_system_contains_apps_and_fonts_without_character_skins_and_rebuilds_identically() {
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
    let (image, report) = build_system(0x1f0000, &packages).unwrap();
    assert_eq!(image.len(), 0x1f0000);
    assert_eq!(report["applications"].as_array().unwrap().len(), 5);
    let tree = snapshot(&image).unwrap();
    assert!(tree.keys().all(|path| path == "/apps"
        || path.starts_with("/apps/")
        || path == "/ui"
        || path.starts_with("/ui/")));
    assert!(!tree.keys().any(|path| path.starts_with("/skins")));
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
    let id = os::apps::lifecycle::uninstall::Identity::new("snake", Origin::Data).unwrap();
    os::apps::lifecycle::uninstall::execute(
        &mut vfs,
        os::apps::lifecycle::uninstall::Operation::Remove(id),
    )
    .unwrap();
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
fn development_examples_are_valid_but_not_factory_apps() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut ids = BTreeSet::new();
    for directory in ["apps/examples"] {
        for app in host_fs::read_dir(root.join(directory)).unwrap() {
            let app = app.unwrap();
            if !app.file_type().unwrap().is_dir() {
                continue;
            }
            let package = Package::read(&app.path()).unwrap();
            assert!(ids.insert(package.id));
        }
    }
    assert_eq!(
        ids,
        BTreeSet::from([String::from("hardware"), String::from("hello")])
    );
    assert!(apps().iter().all(|p| !ids.contains(&p.id)));
}

#[test]
fn standalone_bundle_verifies_actual_payload_and_detects_tampering() {
    let out = std::env::temp_dir().join(format!("dango-app-bundle-{}", std::process::id()));
    assert!(!out.exists());
    let package = apps().remove(0);
    let report = bundle::export(&package, &out).unwrap();
    assert_eq!(bundle::verify(&out).unwrap(), report);
    assert_eq!(
        host_fs::read(out.join("payload/scripts/main.lua")).unwrap(),
        match &package.files["scripts/main.lua"] {
            Entry::File(bytes) => bytes.clone(),
            _ => unreachable!(),
        }
    );
    host_fs::write(out.join("payload/scripts/main.lua"), b"return 999").unwrap();
    assert!(bundle::verify(&out).unwrap_err().contains("inventory"));
    host_fs::remove_dir_all(out).unwrap();
}

#[test]
fn ts_bundle_preserves_binary_payload_and_checks_engine_kind() {
    let out = std::env::temp_dir().join(format!("dango-ts-bundle-{}", std::process::id()));
    assert!(!out.exists());
    // Transport fixture only; executable bytecode is validated by ts-engine.
    let bytecode = b"\0\xffbinary package";
    let package = Package {
        id: "example".into(),
        files: Tree::from([
            (
                "launcher.json".into(),
                Entry::File(br#"{"schema_version":1,"id":"example","entry":"main.etb"}"#.to_vec()),
            ),
            ("main.etb".into(), Entry::File(bytecode.to_vec())),
        ]),
        documentation: Tree::new(),
    };
    let mut report = bundle::export(&package, &out).unwrap();
    assert_eq!(report["kind"], "embedts-application");
    assert_eq!(
        host_fs::read(out.join("payload/main.etb")).unwrap(),
        bytecode
    );
    assert_eq!(bundle::verify(&out).unwrap(), report);
    report["kind"] = "dango-lua-application".into();
    host_fs::write(
        out.join("bundle.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
    assert!(bundle::verify(&out).unwrap_err().contains("engine kind"));
    host_fs::remove_dir_all(out).unwrap();
}

#[cfg(feature = "ts-engine")]
#[test]
fn precompiled_embedts_system_preserves_bytecode_and_registers_a_runnable_application() {
    let bytes = include_bytes!("../../../resources/embedts/selftest.etb");
    let package = Package {
        id: "ts-fixture".into(),
        files: Tree::from([
            (
                "launcher.json".into(),
                Entry::File(
                    br#"{"schema_version":1,"id":"ts-fixture","entry":"scripts/main.etb"}"#
                        .to_vec(),
                ),
            ),
            ("scripts".into(), Entry::Directory),
            ("scripts/main.etb".into(), Entry::File(bytes.to_vec())),
        ]),
        documentation: Tree::new(),
    };
    let (image, report) = build_system(0x1f0000, &[package]).unwrap();
    assert_eq!(report["applications"][0]["id"], "ts-fixture");
    assert_eq!(report["applications"][0]["runnable"], true);
    let tree = snapshot(&image).unwrap();
    assert!(
        matches!(&tree["/apps/ts-fixture/scripts/main.etb"], Entry::File(actual) if actual == bytes)
    );
}
