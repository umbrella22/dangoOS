//! Native loading publication, full entrance, and terminal battery view.
use os::{
    AppMsg, Expression, FoxControl, MosaicoApp, Screen,
    hub::{AppId, Page},
    ui::{splash::SplashControl, theme},
};
use rs_lvgl::{Display, sys};
use rs_lvgl_ui::{App, Ui};
use std::{cell::RefCell, io::Write, ptr, rc::Rc};
fn capture(name: &str, pixels: &[u8]) {
    if let Some(dir) = std::env::var_os("MOSAICO_STARTUP_PREVIEW") {
        std::fs::create_dir_all(&dir).unwrap();
        let mut f =
            std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.ppm"))).unwrap();
        write!(f, "P6\n480 480\n255\n").unwrap();
        f.write_all(pixels).unwrap();
    }
}
#[test]
fn no_builtin_before_skin_publication_and_low_battery_is_terminal() {
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
    let control = FoxControl::new(Expression::Sleep);
    control.set_loading(true);
    control.set_plane_mode(true);
    let mut app = MosaicoApp::new(control.clone(), SplashControl::new());
    let mut ui = Ui::new(&display, theme()).unwrap();
    ui.mount(&app).unwrap();
    let mut expected = vec![0; os::fox::W * os::fox::H];
    os::fox::loading::render(&mut expected);
    for _ in 0..60 {
        ui.tick(50).unwrap();
        let frame = control.take_frame().unwrap();
        assert_eq!(frame.pixels(), expected);
        assert!(
            control.entrance_active(),
            "loading cannot consume the selected character entrance"
        );
        assert!(
            !control.shutdown_sleep_frame_since(0),
            "placeholder cannot acknowledge a Sleep upload"
        );
    }
    let mut loading = vec![0; 480 * 480 * 3];
    for y in 0..os::fox::H {
        for x in 0..os::fox::W {
            let p = expected[y * os::fox::W + x];
            let at = ((y + (480 - os::fox::H) / 2) * 480 + x + (480 - os::fox::W) / 2) * 3;
            loading[at..at + 3].copy_from_slice(&[
                ((p >> 11) * 255 / 31) as u8,
                (((p >> 5) & 63) * 255 / 63) as u8,
                ((p & 31) * 255 / 31) as u8,
            ]);
        }
    }
    capture("loading", &loading);
    // A successful publication, including a deliberate builtin selection,
    // clears loading and only now advances the complete entrance.
    control.set_skin(os::fox::skin::Skin::builtin());
    assert!(!control.loading());
    ui.tick(50).unwrap();
    assert_ne!(control.take_frame().unwrap().pixels(), expected);
    assert!(control.entrance_active());
    for _ in 0..60 {
        ui.tick(50).unwrap();
        drop(control.take_frame());
    }
    assert!(!control.entrance_active());
    assert!(control.take_entrance_finished());
    app.show_low_battery();
    app.update(AppMsg::Open(AppId::Settings));
    app.update(AppMsg::SplashDone);
    app.enter_fox();
    app.go(Page::Launcher);
    assert_eq!(app.screen(), Screen::LowBattery);
    assert_eq!(app.page(), Page::Home);
    assert!(app.take_settings_action().is_none());
    ui.mark_dirty();
    ui.drain(&mut app, 32).unwrap();
    unsafe {
        sys::lv_obj_invalidate(sys::lv_screen_active());
        sys::lv_refr_now(ptr::null_mut());
    }
    let low = pixels.borrow();
    assert!(
        low.chunks_exact(3)
            .filter(|p| p.iter().any(|&c| c > 20))
            .count()
            > 1500
    );
    capture("low-battery", &low);
    drop(low);
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
}
