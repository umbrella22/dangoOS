//! Application manifest, precedence, module and icon regression cases.
use super::*;
use ::fs::{Fs, Geometry, RamDevice, Vfs};
use alloc::{boxed::Box, format, string::String, vec::Vec};

fn vfs() -> Vfs {
    let mut v = Vfs::new();
    for (name, blocks) in [("system", 64), ("data", 64)] {
        let fs = Fs::mount_or_format(
            Box::new(RamDevice::new(4096, blocks)),
            Geometry::flash(blocks),
        )
        .unwrap()
        .0;
        v.mount(name, fs);
    }
    v
}

fn put(v: &mut Vfs, path: &str, data: &str) {
    let dir = path.rsplit_once('/').unwrap().0;
    v.create_dir_all(dir).unwrap();
    v.write_file(path, data.as_bytes()).unwrap();
}

fn manifest(id: &str, extra: &str) -> String {
    format!(r#"{{"schema_version":1,"id":"{id}","entry":"scripts/main.lua"{extra}}}"#)
}

#[test]
fn manifests_follow_the_official_rules() {
    let ok = parse_manifest(
        r#"{"schema_version":1,"id":"dino","entry":"scripts/dino.lua","display_name":"Dino",
                "icon":"assets/icon.jpg","args":{ "a" : [1] },"order":10,"visible":true}"#,
        "dino",
    )
    .unwrap();
    assert_eq!(ok.display_name.as_deref(), Some("Dino"));
    assert_eq!(ok.args_json, r#"{ "a" : [1] }"#);
    assert_eq!((ok.order, ok.visible), (10, true));
    let min = parse_manifest(&manifest("x", ""), "x").unwrap();
    assert_eq!(
        (min.args_json.as_str(), min.order, min.visible, min.icon),
        ("{}", 0, true, None)
    );
    for (bad, dir) in [
        (manifest("x", ""), "y"),
        (
            manifest("x", "").replace("\"schema_version\":1", "\"schema_version\":2"),
            "x",
        ),
        (
            manifest("x", "").replace("\"schema_version\":1", "\"schema_version\":1.5"),
            "x",
        ),
        (
            manifest("x", "").replace("scripts/main.lua", "/abs.lua"),
            "x",
        ),
        (
            manifest("x", "").replace("scripts/main.lua", "../up.lua"),
            "x",
        ),
        (
            manifest("x", "").replace("scripts/main.lua", "main.py"),
            "x",
        ),
        (manifest("x", r#","icon":"a.png""#), "x"),
        (manifest("x", r#","display_name":"""#), "x"),
        (manifest("x", r#","args":[]"#), "x"),
        (manifest("x", r#","order":1.5"#), "x"),
        (manifest("x", r#","visible":1"#), "x"),
        (manifest("a b", ""), "a b"),
        ("[]".into(), "x"),
        ("{".into(), "x"),
    ] {
        assert!(parse_manifest(&bad, dir).is_err(), "{bad}");
    }
    assert!(id_is_valid(&"a".repeat(63)) && !id_is_valid(&"a".repeat(64)));
}

#[test]
fn module_check_sees_hard_requires_only() {
    let src = r#"
            local display = require("display")
            local net, storage = require("net"), require 'storage'
            local ok, button = pcall(require, "button")
            -- local net = require("net")
            local helper = require("helper")
            print("require(button) failed")
            local x = my_require("thing")
            local again = require("net")
        "#;
    let missing = missing_modules(src, &mut |m| m == "helper");
    assert_eq!(missing, ["net"]);
}

#[test]
fn scan_merges_roots_with_shadowing_and_order() {
    let mut v = vfs();
    // /system: a runnable app, one needing lvgl, a hidden one, a broken one.
    put(
        &mut v,
        "/system/apps/dino/launcher.json",
        &manifest("dino", r#","display_name":"Dino","order":10"#),
    );
    put(
        &mut v,
        "/system/apps/dino/scripts/main.lua",
        "local d = require('display')",
    );
    put(
        &mut v,
        "/system/apps/album/launcher.json",
        &manifest("album", r#","order":12"#),
    );
    put(
        &mut v,
        "/system/apps/album/scripts/main.lua",
        "local l = require('lvgl')\nlocal i = require('image')",
    );
    put(
        &mut v,
        "/system/apps/hidden/launcher.json",
        &manifest("hidden", r#","visible":false"#),
    );
    put(&mut v, "/system/apps/hidden/scripts/main.lua", "");
    put(
        &mut v,
        "/system/apps/broken/launcher.json",
        &manifest("other", ""),
    );
    put(
        &mut v,
        "/system/apps/noentry/launcher.json",
        &manifest("noentry", ""),
    );
    put(
        &mut v,
        "/system/scripts/builtin/lib/ssd1306.lua",
        "return {}",
    );
    // System packages are loaded from the filesystem, including both games.
    for (id, manifest, script, icon) in [
        (
            "2048",
            include_str!("../../../../../apps/games/2048/launcher.json"),
            include_str!("../../../../../apps/games/2048/scripts/main.lua"),
            include_bytes!("../../../../../apps/games/2048/assets/icon.jpg").as_slice(),
        ),
        (
            "snake",
            include_str!("../../../../../apps/games/snake/launcher.json"),
            include_str!("../../../../../apps/games/snake/scripts/main.lua"),
            include_bytes!("../../../../../apps/games/snake/assets/icon.jpg").as_slice(),
        ),
    ] {
        put(
            &mut v,
            &format!("/system/apps/{id}/launcher.json"),
            manifest,
        );
        put(
            &mut v,
            &format!("/system/apps/{id}/scripts/main.lua"),
            script,
        );
        v.create_dir_all(&format!("/system/apps/{id}/assets"))
            .unwrap();
        v.write_file(&format!("/system/apps/{id}/assets/icon.jpg"), icon)
            .unwrap();
    }
    // /data shadows dino and snake, and adds one using a shared library.
    put(
        &mut v,
        "/data/apps/dino/launcher.json",
        &manifest("dino", r#","display_name":"Dino 2","order":10"#),
    );
    put(&mut v, "/data/apps/dino/scripts/main.lua", "");
    put(
        &mut v,
        "/data/apps/oled/launcher.json",
        &manifest("oled", r#","order":-1"#),
    );
    put(
        &mut v,
        "/data/apps/oled/scripts/main.lua",
        "local s = require('ssd1306')",
    );
    put(
        &mut v,
        "/data/apps/snake/launcher.json",
        &manifest("snake", r#","display_name":"My Snake","order":20"#),
    );
    put(&mut v, "/data/apps/snake/scripts/main.lua", "");
    v.create_dir_all("/data/apps/.incoming/x").unwrap();

    let mut logs = Vec::new();
    let mut icons = IconCache::new();
    let reg = Registry::scan(&mut v, &mut icons, &mut |l| logs.push(String::from(l)));
    let ids: Vec<&str> = reg.apps.iter().map(|a| a.app.id.as_str()).collect();
    assert_eq!(ids, ["oled", "dino", "album", "2048", "snake"], "{logs:?}");
    let dino = &reg.apps[reg.find("dino").unwrap()];
    assert_eq!((dino.title.as_str(), dino.origin), ("Dino 2", Origin::Data));
    assert_eq!(dino.app.entry, "/data/apps/dino/scripts/main.lua");
    let album = &reg.apps[reg.find("album").unwrap()];
    assert!(album.missing.is_empty());
    assert!(album.runnable() && reg.apps[0].runnable());
    let snake = &reg.apps[reg.find("snake").unwrap()];
    assert_eq!(
        (snake.title.as_str(), snake.origin),
        ("My Snake", Origin::Data)
    );
    let g = &reg.apps[reg.find("2048").unwrap()];
    assert_eq!(g.origin, Origin::System);
    assert_eq!(
        g.app.args_json.split_whitespace().collect::<String>(),
        r#"{"run_time_ms":0}"#
    );
    assert_eq!(g.app.entry, "/system/apps/2048/scripts/main.lua");
    let icon = g.icon.expect("system icon decodes");
    assert!(icon.width <= ICON_BOX && icon.height <= ICON_BOX && icon.width >= 48);
    for needle in [
        "dino in /system/apps is shadowed",
        "broken",
        "noentry",
        "snake in /system/apps is shadowed",
    ] {
        assert!(
            logs.iter().any(|l| l.contains(needle)),
            "{needle}: {logs:?}"
        );
    }
    assert!(!logs.iter().any(|l| l.contains("hidden")), "{logs:?}");
}

#[test]
fn no_lua_apps_without_a_file_system() {
    let reg = Registry::scan(&mut Vfs::new(), &mut IconCache::new(), &mut |_| {});
    assert!(reg.apps.is_empty());
}

#[test]
fn icons_scale_into_the_box_and_are_cached() {
    let img = lua_engine::jpeg::Rgb888 {
        width: 48,
        height: 24,
        pixels: alloc::vec![255; 48 * 24 * 3],
    };
    let i = scale_to_box(&img);
    assert_eq!((i.width, i.height, i.rgb565.len()), (96, 48, 96 * 48 * 2));
    assert_eq!(&i.rgb565[..2], &[0xFF, 0xFF]);
    let big = lua_engine::jpeg::Rgb888 {
        width: 200,
        height: 100,
        pixels: alloc::vec![0; 200 * 100 * 3],
    };
    let b = scale_to_box(&big);
    assert_eq!((b.width, b.height), (96, 48));
    let mut cache = IconCache::new();
    let a = cache
        .get(include_bytes!(
            "../../../../../apps/games/snake/assets/icon.jpg"
        ))
        .unwrap();
    let again = cache
        .get(include_bytes!(
            "../../../../../apps/games/snake/assets/icon.jpg"
        ))
        .unwrap();
    assert!(core::ptr::eq(a.rgb565, again.rgb565));
    assert!(cache.get(b"nope").is_none());
}
