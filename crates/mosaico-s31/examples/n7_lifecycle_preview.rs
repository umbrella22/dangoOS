//! Real pointer/LVGL journey for N7. No device access.
use core::ptr;
use lvgl::{sys, Display};
use lvgl_ui::Ui;
use mosaico_s31::hub::{
    gesture::SystemGesture,
    registry::{IconCache, Origin, Registry},
    uninstall::{Identity, Operation},
    Page,
};
use mosaico_s31::ui::{splash::SplashControl, theme, AppMsg, MosaicoApp};
use mosaico_s31::{Expression, FoxControl};
use std::{
    cell::RefCell,
    fs,
    io::Write,
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    time::Duration,
};
unsafe extern "C" {
    static lv_label_class: sys::lv_obj_class_t;
}
const W: u32 = 480;
const H: u32 = 480;
static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);
static DOWN: AtomicBool = AtomicBool::new(false);
unsafe extern "C" fn read(_i: *mut sys::lv_indev_t, d: *mut sys::lv_indev_data_t) {
    let d = unsafe { &mut *d };
    d.point.x = X.load(Ordering::Relaxed);
    d.point.y = Y.load(Ordering::Relaxed);
    d.state = if DOWN.load(Ordering::Relaxed) {
        sys::lv_indev_state_t_LV_INDEV_STATE_PRESSED
    } else {
        sys::lv_indev_state_t_LV_INDEV_STATE_RELEASED
    };
}
fn run(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, ms: u32) {
    for _ in 0..ms.div_ceil(10) {
        lvgl::tick_inc(Duration::from_millis(10));
        ui.tick(10).unwrap();
        ui.timer_handler();
        ui.drain(app, 32).unwrap();
    }
}
fn press(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, x: i32, y: i32, ms: u32) {
    X.store(x, Ordering::Relaxed);
    Y.store(y, Ordering::Relaxed);
    DOWN.store(true, Ordering::Relaxed);
    run(ui, app, ms);
}
fn release(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp) {
    DOWN.store(false, Ordering::Relaxed);
    run(ui, app, 120);
}
fn tap(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, x: i32, y: i32) {
    press(ui, app, x, y, 120);
    release(ui, app);
}
fn shot(dir: &Path, name: &str, pixels: &Rc<RefCell<Vec<u8>>>) {
    unsafe {
        sys::lv_obj_invalidate(sys::lv_screen_active());
        sys::lv_refr_now(ptr::null_mut());
    }
    let mut f = fs::File::create(dir.join(format!("{name}.ppm"))).unwrap();
    write!(f, "P6\n480 480\n255\n").unwrap();
    f.write_all(&pixels.borrow()).unwrap();
    println!("PASS {name}");
}
unsafe fn center_label(obj: *mut sys::lv_obj_t, wanted: &str) -> Option<(i32, i32)> {
    unsafe {
        if sys::lv_obj_check_type(obj, &lv_label_class) {
            let text = std::ffi::CStr::from_ptr(sys::lv_label_get_text(obj));
            if text.to_bytes() == wanted.as_bytes() {
                let mut area = core::mem::zeroed();
                sys::lv_obj_get_coords(sys::lv_obj_get_parent(obj), &mut area);
                return Some(((area.x1 + area.x2) / 2, (area.y1 + area.y2) / 2));
            }
        }
        for i in 0..sys::lv_obj_get_child_count(obj) {
            if let Some(p) = center_label(sys::lv_obj_get_child(obj, i as i32), wanted) {
                return Some(p);
            }
        }
        None
    }
}
fn main() {
    unsafe {
        lvgl::deinit();
    }
    lvgl::init();
    let pixels = Rc::new(RefCell::new(vec![0; 480 * 480 * 3]));
    let out_pixels = pixels.clone();
    let display = Display::register::<_, { 480 * 48 }>(W, H, move |r| {
        let width = (r.area.x2 - r.area.x1 + 1) as usize;
        let height = (r.area.y2 - r.area.y1 + 1) as usize;
        let mut p = out_pixels.borrow_mut();
        for y in 0..height {
            for x in 0..width {
                let c = r.colors[y * width + x];
                let i = ((r.area.y1 as usize + y) * 480 + r.area.x1 as usize + x) * 3;
                p[i] = c.r();
                p[i + 1] = c.g();
                p[i + 2] = c.b();
            }
        }
    })
    .unwrap();
    let indev = unsafe {
        let i = sys::lv_indev_create();
        sys::lv_indev_set_type(i, sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        sys::lv_indev_set_read_cb(i, Some(read));
        i
    };
    let mut ui = Ui::new(&display, theme()).unwrap();
    let mut app = MosaicoApp::new(FoxControl::new(Expression::Idle), SplashControl::new());
    let mut icons = IconCache::new();
    let mut registry = Registry::builtin_only(&mut icons);
    let first_id = registry.apps[0].app.id.clone();
    app.set_registry(registry.clone());
    app.go(Page::Launcher);
    ui.mount(&app).unwrap();
    run(&mut ui, &mut app, 100);
    let dir = Path::new("target/n7-acceptance-20261023/ui");
    fs::create_dir_all(dir).unwrap();
    // Native tiles have no destructive action, even after a full long hold.
    press(&mut ui, &mut app, 104, 140, 800);
    release(&mut ui, &mut app);
    assert!(app.confirmation().is_none());
    assert!(!app.take_management_haptic(), "native hold has no haptic");
    assert_eq!(app.page(), Page::Launcher);
    tap(&mut ui, &mut app, 172, 298);
    assert!(matches!(app.page(), Page::App(_)));
    app.go(Page::Launcher);
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    press(&mut ui, &mut app, 172, 298, 800);
    assert_eq!(app.confirmation().unwrap().id(), first_id);
    assert!(app.take_management_haptic());
    assert!(!app.take_management_haptic());
    release(&mut ui, &mut app);
    assert_eq!(app.page(), Page::Launcher);
    assert!(app.confirmation().is_some());
    shot(dir, "01-builtin-long-release", &pixels);
    ui.post(AppMsg::Gesture(SystemGesture::Back)).unwrap();
    ui.drain(&mut app, 16).unwrap();
    assert!(app.confirmation().is_none());
    registry.apps[0].origin = Origin::System;
    app.set_registry(registry.clone());
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    press(&mut ui, &mut app, 172, 298, 800);
    release(&mut ui, &mut app);
    shot(dir, "02-system-dialog", &pixels);
    tap(&mut ui, &mut app, 20, 240);
    assert!(app.confirmation().is_none());
    assert_eq!(app.page(), Page::Launcher);
    registry.apps[0].origin = Origin::Data;
    app.set_registry(registry.clone());
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    press(&mut ui, &mut app, 172, 298, 800);
    release(&mut ui, &mut app);
    shot(dir, "03-data-shadow-dialog", &pixels);
    let expected = Identity::new(&first_id, Origin::Data).unwrap();
    // Reorder while the dialog is open; confirmation must preserve the ID.
    registry.apps.swap(0, 1);
    app.set_registry(registry.clone());
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    let (x, y) =
        unsafe { center_label(sys::lv_screen_active(), "确认").expect("confirm button") };
    tap(&mut ui, &mut app, x, y);
    assert!(app.confirmation().is_none());
    let op = app
        .take_management()
        .expect("exactly one retained operation");
    assert_eq!(op, Operation::Remove(expected));
    assert!(app.take_management().is_none());
    run(&mut ui, &mut app, 100);
    shot(dir, "04-busy-before-execution", &pixels);
    app.management_finished(op, Ok(()));
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    shot(dir, "05-removal-toast", &pixels);
    // A source change cannot silently reinterpret the queued stable identity.
    registry.apps.swap(0, 1);
    app.set_registry(registry.clone());
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    press(&mut ui, &mut app, 172, 298, 800);
    release(&mut ui, &mut app);
    registry.apps[0].origin = Origin::System;
    app.set_registry(registry);
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    let (x, y) = unsafe { center_label(sys::lv_screen_active(), "确认").unwrap() };
    tap(&mut ui, &mut app, x, y);
    assert!(
        app.take_management().is_none(),
        "changed source must require fresh confirmation"
    );
    shot(dir, "06-source-change-rejected", &pixels);
    // Display text is bounded independently of the complete operation identity.
    let mut long_registry = app.registry().clone();
    long_registry.apps[0].title = "应用\n".repeat(200);
    let long_identity =
        Identity::new(&long_registry.apps[0].app.id, long_registry.apps[0].origin).unwrap();
    app.set_registry(long_registry);
    ui.mark_dirty();
    run(&mut ui, &mut app, 100);
    press(&mut ui, &mut app, 172, 298, 800);
    release(&mut ui, &mut app);
    shot(dir, "07-long-display-name", &pixels);
    let (x, y) = unsafe { center_label(sys::lv_screen_active(), "确认").unwrap() };
    assert!(
        (60..420).contains(&x) && (60..420).contains(&y),
        "confirm remains on screen"
    );
    let cancel = unsafe { center_label(sys::lv_screen_active(), "取消").unwrap() };
    assert!(
        (60..420).contains(&cancel.0) && (60..420).contains(&cancel.1),
        "cancel remains on screen"
    );
    tap(&mut ui, &mut app, x, y);
    assert_eq!(app.take_management(), Some(Operation::Clear(long_identity)));
    run(&mut ui, &mut app, 100);
    shot(dir, "08-long-name-confirmed", &pixels);
    ui.unmount();
    drop(ui);
    unsafe {
        sys::lv_indev_delete(indev);
        display.delete();
    }
}
