//! Render native settings with LVGL and exercise pointer/text input end to end.
use rs_lvgl::{sys, Display};
use rs_lvgl_ui::{App, Ui};
use std::{
    cell::RefCell,
    ffi::{CStr, CString},
    io::Write,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    time::Duration,
};
use {
    os::companion::behavior::Expression, os::companion::widget::FoxControl,
    os::services::settings::Command as Action, os::shell::app::theme,
    os::shell::components::splash::SplashControl, os::shell::navigation::route::AppId,
    os::shell::navigation::route::Page, os::shell::pages::settings::model as settings, os::AppMsg,
    os::MosaicoApp,
};

static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);
static DOWN: AtomicBool = AtomicBool::new(false);
unsafe extern "C" {
    static lv_textarea_class: sys::lv_obj_class_t;
    static lv_keyboard_class: sys::lv_obj_class_t;
    static lv_spinner_class: sys::lv_obj_class_t;
    static lv_label_class: sys::lv_obj_class_t;
    static lv_slider_class: sys::lv_obj_class_t;
    static lv_switch_class: sys::lv_obj_class_t;
}
unsafe extern "C" fn input(_: *mut sys::lv_indev_t, data: *mut sys::lv_indev_data_t) {
    unsafe {
        (*data).point.x = X.load(Ordering::Relaxed);
        (*data).point.y = Y.load(Ordering::Relaxed);
        (*data).state = if DOWN.load(Ordering::Relaxed) {
            sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
        } else {
            sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
        };
    }
}
fn run(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, ms: u32) {
    for _ in 0..ms.div_ceil(10) {
        rs_lvgl::tick_inc(Duration::from_millis(10));
        ui.timer_handler();
        ui.drain(app, 32).unwrap();
    }
}
fn tap(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, x: i32, y: i32) {
    X.store(x, Ordering::Relaxed);
    Y.store(y, Ordering::Relaxed);
    DOWN.store(true, Ordering::Relaxed);
    run(ui, app, 100);
    DOWN.store(false, Ordering::Relaxed);
    run(ui, app, 120);
}
fn swipe_up(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp) {
    X.store(240, Ordering::Relaxed);
    Y.store(375, Ordering::Relaxed);
    DOWN.store(true, Ordering::Relaxed);
    run(ui, app, 40);
    for y in (140..375).step_by(12).rev() {
        Y.store(y, Ordering::Relaxed);
        run(ui, app, 20);
    }
    DOWN.store(false, Ordering::Relaxed);
    run(ui, app, 1000);
}
unsafe fn label(root: *mut sys::lv_obj_t, text: &str) -> Option<*mut sys::lv_obj_t> {
    unsafe {
        if sys::lv_obj_has_class(root, &raw const lv_label_class)
            && CStr::from_ptr(sys::lv_label_get_text(root)).to_bytes() == text.as_bytes()
        {
            return Some(root);
        }
        for i in 0..sys::lv_obj_get_child_count(root) {
            if let Some(found) = label(sys::lv_obj_get_child(root, i as i32), text) {
                return Some(found);
            }
        }
        None
    }
}
unsafe fn find(
    root: *mut sys::lv_obj_t,
    class: *const sys::lv_obj_class_t,
) -> Option<*mut sys::lv_obj_t> {
    unsafe {
        if sys::lv_obj_has_class(root, class) {
            return Some(root);
        }
        for i in 0..sys::lv_obj_get_child_count(root) {
            if let Some(found) = find(sys::lv_obj_get_child(root, i as i32), class) {
                return Some(found);
            }
        }
    }
    None
}
fn tap_label(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, value: &str) {
    unsafe {
        let object = label(sys::lv_screen_active(), value).expect(value);
        sys::lv_obj_scroll_to_view_recursive(object, false);
    }
    run(ui, app, 60);
    let area = unsafe {
        let object = label(sys::lv_screen_active(), value).expect(value);
        let mut area = core::mem::zeroed();
        sys::lv_obj_get_coords(object, &mut area);
        area
    };
    tap(ui, app, (area.x1 + area.x2) / 2, (area.y1 + area.y2) / 2);
}
fn capture(name: &str, pixels: &Rc<RefCell<Vec<u8>>>) {
    if let Some(path) = std::env::var_os("MOSAICO_SETTINGS_PREVIEW") {
        unsafe {
            sys::lv_obj_invalidate(sys::lv_screen_active());
            sys::lv_refr_now(ptr::null_mut());
        }
        std::fs::create_dir_all(&path).unwrap();
        let mut file =
            std::fs::File::create(std::path::Path::new(&path).join(format!("{name}.ppm"))).unwrap();
        write!(file, "P6\n480 480\n255\n").unwrap();
        file.write_all(&pixels.borrow()).unwrap();
    }
}

/// Measure the bright thumb separately from its dark/orange track. A clipped
/// or stretched circle loses its square bounds or its narrow first scanline.
fn round_thumb(pixels: &[u8], crop: sys::lv_area_t) -> Option<sys::lv_area_t> {
    let mut points = Vec::new();
    for y in crop.y1.max(0)..=crop.y2.min(479) {
        for x in crop.x1.max(0)..=crop.x2.min(479) {
            let at = ((y * 480 + x) * 3) as usize;
            if pixels[at..at + 3].iter().all(|v| *v >= 240) {
                points.push((x, y));
            }
        }
    }
    let bounds = sys::lv_area_t {
        x1: points.iter().map(|p| p.0).min()?,
        x2: points.iter().map(|p| p.0).max()?,
        y1: points.iter().map(|p| p.1).min()?,
        y2: points.iter().map(|p| p.1).max()?,
    };
    let (width, height) = (bounds.x2 - bounds.x1 + 1, bounds.y2 - bounds.y1 + 1);
    let first_row = points.iter().filter(|p| p.1 == bounds.y1).count() as i32;
    (width >= 12 && (width - height).abs() <= 1 && first_row * 4 < width * 3).then_some(bounds)
}

fn assert_native_thumbs(pixels: &Rc<RefCell<Vec<u8>>>, has_switch: bool) {
    // SAFETY: these classes and their parents belong to the mounted settings
    // tree on this test thread. Rendering completes before pixels are read.
    unsafe {
        sys::lv_obj_invalidate(sys::lv_screen_active());
        sys::lv_refr_now(ptr::null_mut());
        let classes = [&raw const lv_slider_class, &raw const lv_switch_class];
        for class in classes.into_iter().take(if has_switch { 2 } else { 1 }) {
            let control = find(sys::lv_screen_active(), class).expect("settings control");
            let mut crop: sys::lv_area_t = core::mem::zeroed();
            sys::lv_obj_get_coords(control, &mut crop);
            crop.x1 -= 16;
            crop.x2 += 16;
            crop.y1 -= 16;
            crop.y2 += 16;
            let data = pixels.borrow();
            let bounds = round_thumb(&data, crop).expect("complete circular native thumb");
            let mut parent: sys::lv_area_t = core::mem::zeroed();
            sys::lv_obj_get_coords(sys::lv_obj_get_parent(control), &mut parent);
            assert!(bounds.x1 > parent.x1 && bounds.x2 < parent.x2);
            assert!(bounds.y1 > parent.y1 && bounds.y2 < parent.y2);

            // Negative control: removing the upper half must be detected.
            let mut clipped = data.clone();
            for y in bounds.y1..=(bounds.y1 + bounds.y2) / 2 {
                for x in bounds.x1..=bounds.x2 {
                    let at = ((y * 480 + x) * 3) as usize;
                    clipped[at..at + 3].fill(0);
                }
            }
            assert!(round_thumb(&clipped, crop).is_none());
        }
    }
}

unsafe fn assert_scrollbar_has_its_own_lane(row_label: &str) {
    unsafe {
        let row = sys::lv_obj_get_parent(label(sys::lv_screen_active(), row_label).unwrap());
        let list = sys::lv_obj_get_parent(row);
        let mut horizontal = core::mem::zeroed();
        let mut vertical: sys::lv_area_t = core::mem::zeroed();
        sys::lv_obj_get_scrollbar_area(list, &mut horizontal, &mut vertical);
        assert!(
            vertical.y2 > vertical.y1,
            "the overflowing list has a scrollbar"
        );
        for index in 0..sys::lv_obj_get_child_count(list) {
            let mut area: sys::lv_area_t = core::mem::zeroed();
            sys::lv_obj_get_coords(sys::lv_obj_get_child(list, index as i32), &mut area);
            assert_eq!(area.x1, 40, "controls keep their original left edge");
            assert_eq!(
                area.x2 - area.x1 + 1,
                400,
                "controls keep their original width"
            );
            assert!(area.x2 + 4 < vertical.x1, "scrollbar must clear every card");
        }
        assert_eq!(sys::lv_obj_get_scroll_left(list), 0);
        assert_eq!(sys::lv_obj_get_scroll_right(list), 0);
    }
}

#[test]
fn categories_controls_and_wifi_password_use_native_events() {
    rs_lvgl::init();
    let pixels = Rc::new(RefCell::new(vec![0u8; 480 * 480 * 3]));
    let output = pixels.clone();
    let display = Display::register::<_, { 480 * 48 }>(480, 480, move |r| {
        let width = (r.area.x2 - r.area.x1 + 1) as usize;
        let height = (r.area.y2 - r.area.y1 + 1) as usize;
        let mut data = output.borrow_mut();
        for y in 0..height {
            for x in 0..width {
                let c = r.colors[y * width + x];
                let at = ((y + r.area.y1 as usize) * 480 + x + r.area.x1 as usize) * 3;
                data[at..at + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
            }
        }
    })
    .unwrap();
    let indev = unsafe {
        let p = sys::lv_indev_create();
        sys::lv_indev_set_type(p, sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        sys::lv_indev_set_read_cb(p, Some(input));
        p
    };
    let mut app = MosaicoApp::new(FoxControl::new(Expression::Idle), SplashControl::new());
    app.go(Page::App(AppId::Settings));
    let mut ui = Ui::new(&display, theme()).unwrap();
    ui.mount(&app).unwrap();
    run(&mut ui, &mut app, 100);
    app.settings_session_mut().info.storage_ready = true;
    app.settings_session_mut().skins = vec![
        os::companion::skins::Choice {
            id: "default".into(),
            name: "鸣潮 · 心月狐团子".into(),
        },
        os::companion::skins::Choice {
            id: "eclipse".into(),
            name: "玄灵 · 月蚀".into(),
        },
        os::companion::skins::Choice {
            id: "normal".into(),
            name: "玄灵 · 常态".into(),
        },
        os::companion::skins::Choice {
            id: "black-blue".into(),
            name: "智谱团子".into(),
        },
    ];
    app.settings_session_mut().selected_skin = "eclipse".into();
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(
            label(sys::lv_screen_active(), "玄灵 · 月蚀  ›").is_some(),
            "root displays the active character"
        );
        assert_scrollbar_has_its_own_lane("更换团子");
    }
    capture("01-settings", &pixels);
    tap(&mut ui, &mut app, 240, 124);
    assert_eq!(
        app.settings_page(),
        settings::Page::Character,
        "character chooser is the first row"
    );
    unsafe {
        for name in [
            "鸣潮 · 心月狐团子",
            "玄灵 · 月蚀",
            "玄灵 · 常态",
            "智谱团子",
        ] {
            assert!(
                label(sys::lv_screen_active(), name).is_some(),
                "bundled name is shown in full: {name}"
            );
        }
        let selected =
            sys::lv_obj_get_parent(label(sys::lv_screen_active(), "玄灵 · 月蚀").unwrap());
        assert_eq!(sys::lv_obj_get_style_border_width(selected, 0), 2);
        assert!(sys::lv_color_eq(
            sys::lv_obj_get_style_border_color(selected, 0),
            sys::lv_color_make(255, 76, 1)
        ));
        assert!(label(sys::lv_screen_active(), "选择后立即切换，重启后保留。").is_none());
        assert!(label(sys::lv_screen_active(), "团子虚影").is_none());
    }
    capture("01-character", &pixels);
    tap(&mut ui, &mut app, 240, 248);
    assert_eq!(
        app.take_settings_action(),
        Some(Action::SelectSkin("normal".into()))
    );
    assert_eq!(
        app.settings_session().selected_skin,
        "eclipse",
        "selection waits for durable validation and activation"
    );
    assert_eq!(
        app.settings_session().loading_skin.as_deref(),
        Some("normal")
    );
    unsafe {
        assert!(find(sys::lv_screen_active(), &raw const lv_spinner_class).is_some());
        assert!(label(sys::lv_screen_active(), "切换中").is_some());
    }
    capture("01-character-loading", &pixels);
    // Duplicate input cannot replace an in-flight target before the task runs.
    app.update(AppMsg::SelectSkin("black-blue".into()));
    assert!(app.take_settings_action().is_none());
    app.settings_session_mut().info.busy = true;
    ui.mark_dirty();
    run(&mut ui, &mut app, 50);
    tap(&mut ui, &mut app, 240, 312);
    assert!(app.take_settings_action().is_none());
    app.settings_session_mut().info.busy = false;
    app.settings_session_mut().loading_skin = None;
    app.update(AppMsg::SettingsBack);
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap_label(&mut ui, &mut app, "显示与电源");
    assert_eq!(app.settings_page(), settings::Page::Display);
    tap(&mut ui, &mut app, 240, 166);
    assert!((40..=80).contains(&app.hub().settings.brightness));
    app.settings_session_mut().info.saving = true;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(find(sys::lv_screen_active(), &raw const lv_spinner_class).is_some());
    }
    capture("02-saving", &pixels);
    app.settings_session_mut().info.saving = false;
    app.settings_session_mut().info.saved = true;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(find(sys::lv_screen_active(), &raw const lv_spinner_class).is_none());
        let check =
            label(sys::lv_screen_active(), rs_lvgl_ui::symbol::OK).expect("saved checkmark");
        assert!(sys::lv_color_eq(
            sys::lv_obj_get_style_text_color(check, 0),
            sys::lv_color_make(99, 194, 112)
        ));
        assert!(label(sys::lv_screen_active(), "偏好已保存").is_none());
    }
    capture("02-display", &pixels);
    tap(&mut ui, &mut app, 60, 48);
    tap_label(&mut ui, &mut app, "声音");
    assert_eq!(app.settings_page(), settings::Page::Audio);
    tap(&mut ui, &mut app, 240, 166);
    assert!((40..=80).contains(&app.hub().settings.audio_volume));
    tap(&mut ui, &mut app, 388, 246);
    assert!(app.hub().settings.audio_muted);
    capture("02-audio", &pixels);
    tap(&mut ui, &mut app, 60, 48);
    tap_label(&mut ui, &mut app, "灯光与震动");
    assert_eq!(app.settings_page(), settings::Page::Feedback);
    tap(&mut ui, &mut app, 388, 126);
    tap(&mut ui, &mut app, 388, 202);
    assert!(!app.hub().settings.lights && !app.hub().settings.haptics);
    capture("03-feedback", &pixels);
    tap(&mut ui, &mut app, 60, 48);
    tap_label(&mut ui, &mut app, "无线连接");
    assert_eq!(app.settings_page(), settings::Page::Wireless);
    tap(&mut ui, &mut app, 388, 202);
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Radio(os::services::radio::Command::BtOn))
    );
    capture("04-wireless", &pixels);
    tap(&mut ui, &mut app, 240, 124);
    assert_eq!(app.settings_page(), settings::Page::Wifi);
    capture("05-wifi-off", &pixels);
    tap(&mut ui, &mut app, 388, 126);
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Radio(os::services::radio::Command::WifiOn))
    );
    app.settings_session_mut().info.wifi = os::services::radio::WifiState::Idle;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap(&mut ui, &mut app, 388, 126);
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Radio(os::services::radio::Command::WifiOff))
    );
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    app.settings_session_mut().info.busy = true;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap(&mut ui, &mut app, 388, 126);
    assert!(app.take_settings_action().is_none());
    app.settings_session_mut().info.busy = false;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap(&mut ui, &mut app, 240, 250);
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Radio(os::services::radio::Command::WifiScan))
    );
    let network = os::services::radio::Network {
        ssid: b"example ap".to_vec(),
        channel: 6,
        rssi: -42,
        auth: "WPA2".into(),
    };
    app.settings_session_mut().networks = vec![
        network.clone(),
        os::services::radio::Network {
            ssid: b"a\0b\xff".to_vec(),
            ..network.clone()
        },
    ];
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    capture("05-wifi", &pixels);
    app.update(AppMsg::WifiChoose(network));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    assert_eq!(app.settings_page(), settings::Page::Password);
    unsafe {
        let root = sys::lv_screen_active();
        let area = find(root, &raw const lv_textarea_class).expect("native textarea");
        assert!(!sys::lv_textarea_get_password_mode(area));
        assert!(find(root, &raw const lv_keyboard_class).is_some());
        let text = CString::new("short").unwrap();
        sys::lv_textarea_set_text(area, text.as_ptr());
    }
    run(&mut ui, &mut app, 100);
    tap(&mut ui, &mut app, 345, 199);
    assert_eq!(app.settings_page(), settings::Page::Password);
    assert!(app.take_settings_action().is_none());
    assert!(app.settings_session().message.is_some());
    capture("06-password-error", &pixels);
    unsafe {
        let area = find(sys::lv_screen_active(), &raw const lv_textarea_class).unwrap();
        let text = CString::new("test-key8").unwrap();
        sys::lv_textarea_set_text(area, text.as_ptr());
    }
    run(&mut ui, &mut app, 100);
    capture("07-password", &pixels);
    tap(&mut ui, &mut app, 396, 138);
    unsafe {
        let area = find(sys::lv_screen_active(), &raw const lv_textarea_class).unwrap();
        assert!(sys::lv_textarea_get_password_mode(area));
        assert_eq!(
            CStr::from_ptr(sys::lv_textarea_get_text(area)).to_bytes(),
            b"test-key8"
        );
    }
    capture("07-password-hidden", &pixels);
    tap(&mut ui, &mut app, 396, 138);
    unsafe {
        let area = find(sys::lv_screen_active(), &raw const lv_textarea_class).unwrap();
        assert!(!sys::lv_textarea_get_password_mode(area));
        assert_eq!(
            CStr::from_ptr(sys::lv_textarea_get_text(area)).to_bytes(),
            b"test-key8"
        );
    }
    tap(&mut ui, &mut app, 345, 199);
    let Some(Action::Radio(os::services::radio::Command::WifiConnect(c))) =
        app.take_settings_action()
    else {
        panic!("connect event")
    };
    assert_eq!(c.ssid, b"example ap");
    assert_eq!(c.password, b"test-key8");
    assert!(app.settings_session().password.0.is_empty());
    assert_eq!(app.settings_page(), settings::Page::Wifi);
    app.settings_session_mut().wifi_saved = os::services::radio::known::Snapshot {
        loaded: true,
        enabled: true,
        automatic: true,
        networks: vec![b"example ap".to_vec()],
        current: Some(b"example ap".to_vec()),
        ..Default::default()
    };
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    capture("05-wifi-saved", &pixels);
    tap_label(&mut ui, &mut app, "example ap");
    assert_eq!(app.settings_page(), settings::Page::KnownNetwork);
    unsafe {
        assert!(find(sys::lv_screen_active(), &raw const lv_textarea_class).is_none());
    }
    tap_label(&mut ui, &mut app, "连接此网络");
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Radio(
            os::services::radio::Command::WifiSavedConnect(b"example ap".to_vec())
        ))
    );
    app.update(AppMsg::WifiChoose(os::services::radio::Network {
        ssid: b"example ap".to_vec(),
        channel: 0,
        rssi: 0,
        auth: "WPA2".into(),
    }));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    capture("05-wifi-known-detail", &pixels);
    tap_label(&mut ui, &mut app, "重新输入密码");
    assert_eq!(app.settings_page(), settings::Page::Password);
    assert!(app.settings_session().password.0.is_empty());
    app.update(AppMsg::SettingsBack);
    app.update(AppMsg::WifiChoose(os::services::radio::Network {
        ssid: b"example ap".to_vec(),
        channel: 0,
        rssi: 0,
        auth: "WPA2".into(),
    }));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap_label(&mut ui, &mut app, "忘记此网络");
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Radio(os::services::radio::Command::WifiForget(
            b"example ap".to_vec()
        )))
    );
    assert_eq!(app.settings_page(), settings::Page::Wifi);
    app.update(AppMsg::SettingsPage(settings::Page::Camera));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap(&mut ui, &mut app, 388, 126);
    assert!(app.hub().settings.camera_mirror);
    capture("08-camera", &pixels);
    tap(&mut ui, &mut app, 240, 384);
    assert_eq!(app.hub().settings.jpeg_quality, 95);
    app.settings_session_mut().info.storage_ready = true;
    app.settings_session_mut().info.data_total_kib = 91040;
    app.settings_session_mut().info.data_free_kib = 90900;
    for (page, name) in [
        (settings::Page::Storage, "09-storage"),
        (settings::Page::System, "10-system"),
    ] {
        app.update(AppMsg::SettingsPage(page));
        ui.mark_dirty();
        run(&mut ui, &mut app, 100);
        if page == settings::Page::System {
            // Opening About must stay interactive before the service has a
            // heap sample. A real zero sample is different from pending data.
            unsafe {
                assert!(label(sys::lv_screen_active(), "读取中").is_some());
            }
            run(&mut ui, &mut app, 1200);
            tap(&mut ui, &mut app, 60, 48);
            assert_eq!(app.settings_page(), settings::Page::Root);
            tap_label(&mut ui, &mut app, "关于设备");
            assert_eq!(app.settings_page(), settings::Page::System);
            for (uptime_s, free_kib, value) in [
                (60, 0, "0.0 MB"),
                (120, 12 * 1024, "12.0 MB"),
                (180, 14 * 1024 + 512, "14.5 MB"),
            ] {
                app.hub_mut().about.uptime_s = uptime_s;
                app.hub_mut().about.free_kib = Some(free_kib);
                ui.mark_dirty();
                run(&mut ui, &mut app, 1100);
                unsafe {
                    assert!(label(sys::lv_screen_active(), value).is_some());
                    assert!(label(sys::lv_screen_active(), "读取中").is_none());
                }
            }
            tap(&mut ui, &mut app, 60, 48);
            assert_eq!(app.settings_page(), settings::Page::Root);
            tap_label(&mut ui, &mut app, "关于设备");
            assert_eq!(app.settings_page(), settings::Page::System);
            unsafe {
                assert!(label(sys::lv_screen_active(), "14.5 MB").is_some());
            }
        }
        capture(name, &pixels);
        app.update(AppMsg::Gesture(
            os::shell::navigation::gesture::SystemGesture::Back,
        ));
        assert_eq!(app.settings_page(), settings::Page::Root);
    }
    app.update(AppMsg::SettingsPage(settings::Page::Update));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    tap(&mut ui, &mut app, 388, 196);
    assert!(!app.hub().settings.auto_update_check);
    capture("16-update-idle", &pixels);
    tap_label(&mut ui, &mut app, "检查更新");
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Update(os::services::update::Command::Check))
    );
    app.settings_session_mut().update = os::services::update::Snapshot {
        phase: os::services::update::Phase::Available,
        version: Some("0.2.2".into()),
        ..Default::default()
    };
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    swipe_up(&mut ui, &mut app);
    capture("17-update-available", &pixels);
    tap_label(&mut ui, &mut app, "下载并安装");
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Update(os::services::update::Command::Install))
    );
    app.settings_session_mut().update.phase = os::services::update::Phase::Downloading;
    app.settings_session_mut().update.received = 256 * 1024;
    app.settings_session_mut().update.total = 1024 * 1024;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    swipe_up(&mut ui, &mut app);
    capture("18-update-downloading", &pixels);
    tap_label(&mut ui, &mut app, "取消更新");
    assert_eq!(
        app.take_settings_action(),
        Some(Action::Update(os::services::update::Command::Cancel))
    );
    app.settings_session_mut().update.phase = os::services::update::Phase::Incompatible;
    app.settings_session_mut().update.error = Some("发布固件使用旧分区布局，已拒绝安装");
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(label(sys::lv_screen_active(), "下载并安装").is_none());
    }
    capture("19-update-incompatible", &pixels);

    app.update(AppMsg::SettingsPage(settings::Page::Proxy));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        let textarea = find(sys::lv_screen_active(), &raw const lv_textarea_class).unwrap();
        sys::lv_textarea_set_text(textarea, CString::new("10.26.104.1:7777").unwrap().as_ptr());
    }
    run(&mut ui, &mut app, 100);
    assert_eq!(app.settings_session().proxy_address, "10.26.104.1:7777");
    capture("20-update-proxy", &pixels);
    tap(&mut ui, &mut app, 342, 199);
    assert_eq!(app.settings_page(), settings::Page::Update);
    assert!(app.hub().settings.update_proxy_enabled);
    assert_eq!(
        app.hub().settings.update_proxy.unwrap().address(),
        "10.26.104.1:7777"
    );
    app.update(AppMsg::UpdateProxyToggle(false));
    assert!(!app.hub().settings.update_proxy_enabled);
    assert!(app.hub().settings.update_proxy.is_some());

    app.go(Page::App(AppId::Camera));
    app.hub_mut().camera.state = os::services::camera::State::Fault;
    app.hub_mut().camera.error = Some("camera-short-frame");
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    capture("11-camera-retry", &pixels);
    tap(&mut ui, &mut app, 240, 394);
    assert_eq!(
        app.take_camera_action(),
        Some(os::services::camera::Command::Start)
    );
    app.hub_mut().camera.state = os::services::camera::State::Preview;
    app.hub_mut().camera.error = None;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    capture("12-camera-ready", &pixels);
    tap(&mut ui, &mut app, 240, 394);
    assert_eq!(
        app.take_camera_action(),
        Some(os::services::camera::Command::Capture)
    );
    app.update(AppMsg::Gesture(
        os::shell::navigation::gesture::SystemGesture::Back,
    ));
    assert_eq!(app.page(), Page::Launcher);

    app.go(Page::App(AppId::Sensors));
    app.hub_mut().snapshot.left = os::services::input::snapshot::SlotState::Module(
        os::companion::behavior::affect::ModuleKind::Camera,
    );
    app.hub_mut().snapshot.accel_mg = Some([0, 0, 1000]);
    app.hub_mut().snapshot.gyro_dps = Some([1, 2, 3]);
    app.hub_mut().snapshot.battery = Some(87);
    app.hub_mut().camera.state = os::services::camera::State::Off;
    app.hub_mut().camera.pid = 0xda4a;
    app.hub_mut().radio = os::services::radio::Snapshot {
        wifi: os::services::radio::WifiState::Connected,
        wifi_link: os::services::radio::WifiLink::Connected {
            rssi_dbm: Some(-55),
        },
        bluetooth: os::services::radio::BluetoothState::Discoverable,
        ip: Some([192, 0, 2, 42]),
    };
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(label(sys::lv_screen_active(), "待机").is_some());
        assert!(label(sys::lv_screen_active(), "SC101IOT").is_some());
    }
    capture("13-sensors", &pixels);
    swipe_up(&mut ui, &mut app);
    let grid = unsafe {
        let wifi = label(sys::lv_screen_active(), "Wi-Fi").unwrap();
        let grid = sys::lv_obj_get_parent(sys::lv_obj_get_parent(wifi));
        assert!(sys::lv_obj_get_scroll_y(grid) > 50);
        assert!(label(sys::lv_screen_active(), "192.0.2.42").is_some());
        assert!(label(sys::lv_screen_active(), "可被发现").is_some());
        grid
    };
    capture("14-sensors-wireless", &pixels);
    let scroll = unsafe { sys::lv_obj_get_scroll_y(grid) };
    app.hub_mut().radio.wifi = os::services::radio::WifiState::Off;
    app.hub_mut().radio.bluetooth = os::services::radio::BluetoothState::Connected;
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(label(sys::lv_screen_active(), "已关闭").is_some());
        assert!(label(sys::lv_screen_active(), "192.0.2.42").is_none());
        assert_eq!(
            sys::lv_obj_get_scroll_y(grid),
            scroll,
            "status refresh preserves scroll"
        );
    }
    capture("15-sensors-updated", &pixels);

    // A fallback can still be the active renderer, but it is not a choice.
    app.go(Page::App(AppId::Settings));
    app.settings_session_mut().skins.clear();
    app.settings_session_mut().selected_skin = os::companion::skins::PLACEHOLDER_ID.into();
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(label(sys::lv_screen_active(), "团子虚影  ›").is_some());
    }
    app.update(AppMsg::SettingsPage(settings::Page::Character));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(label(sys::lv_screen_active(), "团子虚影").is_none());
        assert!(label(sys::lv_screen_active(), "安装团子形象后可在这里切换").is_some());
    }
    app.settings_session_mut()
        .skins
        .push(os::companion::skins::Choice {
            id: "default".into(),
            name: "鸣潮 · 心月狐团子".into(),
        });
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    unsafe {
        assert!(label(sys::lv_screen_active(), "鸣潮 · 心月狐团子").is_some());
        assert!(label(sys::lv_screen_active(), "安装团子形象后可在这里切换").is_none());
    }
    // Exercise the real page components at both endpoints and a middle value.
    // The native renderer, rather than a style-field comparison, proves the
    // circular thumbs and the parent clipping boundary after the API migration.
    for (page, values, name) in [
        (settings::Page::Display, [10, 50, 100], "display"),
        (settings::Page::Audio, [0, 50, 100], "audio"),
    ] {
        app.set_settings_page(page);
        for value in values {
            if page == settings::Page::Display {
                app.hub_mut().settings.set_brightness(value);
            } else {
                app.hub_mut().settings.audio_volume = value as u8;
            }
            ui.update(&app).unwrap();
            run(&mut ui, &mut app, 100);
            capture(&format!("21-{name}-{value}"), &pixels);
            assert_native_thumbs(&pixels, page == settings::Page::Audio);
        }
    }
    ui.unmount();
    drop(ui);
    unsafe {
        sys::lv_indev_delete(indev);
        display.delete();
        rs_lvgl::deinit();
    }
}
