use rs_lvgl::Display;

use super::*;
use crate::test_support;

#[test]
fn the_app_starts_on_the_configured_first_screen() {
    let _guard = test_support::lock();

    // SAFETY: the test lock is the only LVGL user in this test binary.
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 64 * 64 }>(64, 64, |_| {}).expect("memory display");

    let mut ui = rs_lvgl_ui::Ui::new(&display, theme()).expect("Ui::new");
    let control = FoxControl::new(Expression::Idle);
    let splash = SplashControl::new();
    let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
    ui.mount(&app).expect("initial screen mounts");

    if SHOW_SPLASH {
        assert_eq!(app.screen(), Screen::Splash);
        assert_eq!(splash.create_count(), 1);
        assert_eq!(control.create_count(), 0, "the fox is not mounted yet");

        for _ in 0..3 {
            ui.tick(16).expect("Ui::tick");
        }
        assert!(splash.frames() > 0, "the splash rasterizes");

        // A skip message swaps the tree: splash deleted, fox created once.
        ui.post(AppMsg::SplashDone).expect("post");
        ui.drain(&mut app, 8).expect("drain");
        assert_eq!(app.screen(), Screen::Fox);
        assert_eq!(control.create_count(), 1, "one fox widget, no remount");
        assert!(control.entrance_active(), "the fox starts its entrance");

        // Later reconciles keep the fox instance.
        ui.tick(16).expect("tick");
        ui.drain(&mut app, 8).expect("drain");
        assert_eq!(control.create_count(), 1);
    } else {
        assert_eq!(app.screen(), Screen::Fox);
        assert_eq!(splash.create_count(), 0, "the splash is disabled");
        assert_eq!(control.create_count(), 1, "the fox mounts directly");
        assert!(control.entrance_active(), "the entrance starts at mount");
    }

    // Advance past the entrance: it finishes and reports the edge.
    let mut elapsed = 0u32;
    while elapsed < entrance::TOTAL_MS as u32 + 100 {
        ui.tick(50).expect("tick");
        elapsed += 50;
    }
    assert!(!control.entrance_active(), "the entrance completed");
    assert!(control.take_entrance_finished());

    ui.unmount();
    assert_eq!(control.delete_count(), 1, "unmount deletes the fox once");
    assert_eq!(ui.stats().mounted_nodes, 0);

    drop(ui);
    // SAFETY: the UI is unmounted; no LVGL object for this display remains.
    unsafe { display.delete() };
    // SAFETY: the display is gone and this test is the last LVGL user.
    unsafe { rs_lvgl::deinit() };
}

#[test]
fn the_touch_hold_off_covers_the_active_animations() {
    let expected = if SHOW_SPLASH {
        splash::TOTAL_MS + entrance::TOTAL_MS
    } else {
        entrance::TOTAL_MS
    };
    assert_eq!(MosaicoApp::touch_hold_off_ms(), expected);
    assert!(MosaicoApp::touch_hold_off_ms() >= entrance::TOTAL_MS);
}

#[test]
fn the_ai_state_ring_still_cycles() {
    let mut state = Expression::Idle;
    for _ in 0..AI_STATES.len() {
        state = next_ai_state(state);
    }
    assert_eq!(state, Expression::Idle);
    assert_eq!(
        next_ai_state(Expression::Tired),
        Expression::Idle,
        "an expression outside the ring returns to Idle"
    );
}

// ---- a simulated touch panel for the hub pages ----

use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static TOUCH_X: AtomicI32 = AtomicI32::new(0);
static TOUCH_Y: AtomicI32 = AtomicI32::new(0);
static TOUCH_DOWN: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn read_touch(
    _indev: *mut rs_lvgl::sys::lv_indev_t,
    data: *mut rs_lvgl::sys::lv_indev_data_t,
) {
    let data = unsafe { &mut *data };
    data.point.x = TOUCH_X.load(Ordering::Relaxed);
    data.point.y = TOUCH_Y.load(Ordering::Relaxed);
    data.state = if TOUCH_DOWN.load(Ordering::Relaxed) {
        rs_lvgl::sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        rs_lvgl::sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
}

fn run(ui: &mut rs_lvgl_ui::Ui<AppMsg>, app: &mut MosaicoApp, ms: u32) {
    let mut t = 0;
    while t < ms {
        rs_lvgl::tick_inc(core::time::Duration::from_millis(10));
        ui.timer_handler();
        ui.drain(app, 16).expect("drain");
        t += 10;
    }
}

fn tap(ui: &mut rs_lvgl_ui::Ui<AppMsg>, app: &mut MosaicoApp, x: i32, y: i32) {
    TOUCH_X.store(x, Ordering::Relaxed);
    TOUCH_Y.store(y, Ordering::Relaxed);
    TOUCH_DOWN.store(true, Ordering::Relaxed);
    run(ui, app, 120);
    TOUCH_DOWN.store(false, Ordering::Relaxed);
    run(ui, app, 120);
}

#[test]
fn a_tap_anywhere_on_a_launcher_tile_opens_its_app() {
    let _guard = test_support::lock();
    // SAFETY: the test lock is the only LVGL user in this test binary.
    unsafe { rs_lvgl::deinit() };
    rs_lvgl::init();
    let display = Display::register::<_, { 480 * 48 }>(480, 480, |_| {}).expect("memory display");
    // SAFETY: LVGL is initialized and only this thread uses it.
    let indev = unsafe {
        let indev = rs_lvgl::sys::lv_indev_create();
        rs_lvgl::sys::lv_indev_set_type(indev, rs_lvgl::sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        rs_lvgl::sys::lv_indev_set_read_cb(indev, Some(read_touch));
        indev
    };

    let mut ui = rs_lvgl_ui::Ui::new(&display, theme()).expect("Ui::new");
    let control = FoxControl::new(Expression::Idle);
    let splash = SplashControl::new();
    let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
    fn system_apps() -> Registry {
        use {
            crate::apps::registry::AppEntry, crate::apps::registry::LuaApp,
            crate::apps::registry::Origin,
        };
        Registry {
            apps: ["2048", "snake"]
                .into_iter()
                .map(|id| AppEntry {
                    app: LuaApp {
                        id: id.into(),
                        entry: alloc::format!("/system/apps/{id}/scripts/main.lua"),
                        args_json: "{}".into(),
                    },
                    title: id.into(),
                    order: 20,
                    origin: Origin::System,
                    dir: alloc::format!("/system/apps/{id}"),
                    icon: None,
                    missing: Vec::new(),
                })
                .collect(),
        }
    }
    // Two filesystem apps follow the four native Rust applications.
    app.set_registry(system_apps());
    ui.mount(&app).expect("mount");

    // Tile centres: rows of three 120 px columns, 16 px apart, centred;
    // the 96 px icon square is the upper part, the name the lower part.
    let tiles = [
        (104, 140, AppId::Settings),
        (240, 140, AppId::Sensors),
        (376, 140, AppId::Timer),
        (104, 298, AppId::Camera),
        (240, 298, AppId::Lua(0)),
        (376, 298, AppId::Lua(1)),
    ];
    for (x, icon_y, id) in tiles {
        for y in [icon_y, icon_y + 72] {
            app.go(Page::Launcher);
            ui.mark_dirty();
            run(&mut ui, &mut app, 60);
            tap(&mut ui, &mut app, x, y);
            assert_eq!(app.page(), Page::App(id), "tap at ({x}, {y})");
        }
    }

    // More than two rows: the grid scrolls from the top.
    let mut many = system_apps();
    for n in 0..5 {
        let mut e = many.apps[0].clone();
        e.app.id = alloc::format!("extra{n}");
        e.icon = None;
        many.apps.push(e);
    }
    app.set_registry(many);
    app.go(Page::Launcher);
    ui.mark_dirty();
    run(&mut ui, &mut app, 60);
    tap(&mut ui, &mut app, 104, 132);
    assert_eq!(app.page(), Page::App(AppId::Settings), "scrolling grid");
    app.set_registry(system_apps());

    // Settings: display, audio and feedback controls all dispatch to the hub.
    app.go(Page::App(AppId::Settings));
    ui.mark_dirty();
    run(&mut ui, &mut app, 60);
    tap(&mut ui, &mut app, 240, 196);
    assert_eq!(
        app.settings_page(),
        crate::shell::pages::settings::model::Page::Display
    );
    tap(&mut ui, &mut app, 240, 166);
    let brightness = app.hub().settings.brightness;
    assert!(
        (40..=80).contains(&brightness),
        "slider tap set {brightness}%"
    );
    tap(&mut ui, &mut app, 60, 48);
    tap(&mut ui, &mut app, 240, 268);
    assert_eq!(
        app.settings_page(),
        crate::shell::pages::settings::model::Page::Audio
    );
    tap(&mut ui, &mut app, 240, 166);
    assert!((40..=80).contains(&app.hub().settings.audio_volume));
    tap(&mut ui, &mut app, 388, 240);
    assert!(app.hub().settings.audio_muted, "the mute switch toggles");
    tap(&mut ui, &mut app, 60, 48);
    tap(&mut ui, &mut app, 240, 340);
    assert_eq!(
        app.settings_page(),
        crate::shell::pages::settings::model::Page::Feedback
    );
    tap(&mut ui, &mut app, 388, 126);
    assert!(!app.hub().settings.lights, "the lights switch toggles");
    tap(&mut ui, &mut app, 388, 202);
    assert!(!app.hub().settings.haptics, "the haptics switch toggles");

    // Timer: pick the 1-minute chip, then start.
    app.go(Page::App(AppId::Timer));
    ui.mark_dirty();
    run(&mut ui, &mut app, 60);
    tap(&mut ui, &mut app, 88, 334);
    assert_eq!(
        app.hub().countdown.preset(),
        0,
        "the first chip selects 1 min"
    );
    tap(&mut ui, &mut app, 154, 390);
    assert_eq!(
        app.hub().countdown.phase(),
        crate::services::timer::Phase::Running,
        "the primary pill starts the countdown"
    );

    // A tap between tiles does nothing.
    app.go(Page::Launcher);
    ui.mark_dirty();
    run(&mut ui, &mut app, 60);
    tap(&mut ui, &mut app, 240, 60);
    assert_eq!(app.page(), Page::Launcher);

    ui.unmount();
    drop(ui);
    // SAFETY: nothing references the input device or the display anymore.
    unsafe {
        rs_lvgl::sys::lv_indev_delete(indev);
        display.delete();
        rs_lvgl::deinit();
    }
}
