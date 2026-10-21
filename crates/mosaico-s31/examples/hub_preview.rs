//! Headless host preview of the hub pages (RFC 0006).
//!
//! ```text
//! cargo run -p mosaico-s31 --example hub_preview --offline
//! ```
//!
//! Mounts the real [`MosaicoApp`] on a 480x480 memory display, drives it with
//! the same messages the device sends (gestures, taps, timer controls) and
//! writes one PPM per page under `target/hub-preview/`.

use core::ptr;
use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use lvgl::sys as lvgl_sys;
use lvgl::Display;
use lvgl_ui::Ui;
use mosaico_s31::behavior::affect::ModuleKind;
use mosaico_s31::hub::gesture::SystemGesture;
use mosaico_s31::hub::snapshot::{MagnetState, SlotState};
use mosaico_s31::hub::{AppId, Page};
use mosaico_s31::ui::splash::SplashControl;
use mosaico_s31::ui::{theme, AppMsg, MosaicoApp};
use mosaico_s31::{Expression, FoxControl};

const W: u32 = 480;
const H: u32 = 480;
const BPP: usize = 3;
const FRAME_MS: u32 = 16;

type Capture = Rc<RefCell<Vec<u8>>>;

fn main() {
    // SAFETY: single-threaded process that owns LVGL for its lifetime.
    unsafe { lvgl::deinit() };
    lvgl::init();

    let pixels: Capture = Rc::new(RefCell::new(vec![0u8; (W * H) as usize * BPP]));
    let display = register_capturing_display(Rc::clone(&pixels));
    let mut ui = Ui::new(&display, theme()).expect("ui");

    let control = FoxControl::new(Expression::Idle);
    let splash = SplashControl::new();
    let mut app = MosaicoApp::new(Rc::clone(&control), Rc::clone(&splash));
    ui.mount(&app).expect("mount");

    // Sample sensor data for the sensors page.
    {
        let hub = app.hub_mut();
        hub.snapshot.accel_mg = Some([182, -40, 981]);
        hub.snapshot.gyro_dps = Some([2, -1, 0]);
        hub.snapshot.light = Some(42);
        hub.snapshot.presence = Some(true);
        hub.snapshot.battery = Some(87);
        hub.snapshot.left = SlotState::Module(ModuleKind::Interaction);
        hub.snapshot.right = SlotState::Empty;
        hub.snapshot.magnet = MagnetState::Near;
        hub.about.uptime_s = 12 * 60 + 5;
        hub.about.free_kib = 6120;
    }

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/hub-preview");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).expect("output dir");

    let mut step = |ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, msgs: &[AppMsg], name: &str| {
        for &msg in msgs {
            ui.post(msg).expect("post");
        }
        ui.drain(app, 16).expect("drain");
        for _ in 0..40 {
            lvgl::tick_inc(Duration::from_millis(FRAME_MS as u64));
            ui.tick(FRAME_MS).expect("tick");
            ui.timer_handler();
        }
        refresh_full_screen();
        let path = out.join(format!("{name}.ppm"));
        write_ppm(&path, &pixels.borrow());
        println!("{:<20} page={:?} -> {}", name, app.page(), path.display());
    };

    step(&mut ui, &mut app, &[AppMsg::Gesture(SystemGesture::Up)], "1-launcher");
    assert_eq!(app.page(), Page::Launcher);
    step(&mut ui, &mut app, &[AppMsg::Open(AppId::Settings)], "2-settings");
    step(
        &mut ui,
        &mut app,
        &[AppMsg::Brightness(64), AppMsg::Haptics(false)],
        "3-settings-changed",
    );
    step(
        &mut ui,
        &mut app,
        &[AppMsg::Gesture(SystemGesture::Back), AppMsg::Open(AppId::Sensors)],
        "4-sensors",
    );
    step(
        &mut ui,
        &mut app,
        &[AppMsg::Gesture(SystemGesture::Back), AppMsg::Open(AppId::Timer)],
        "5-timer-ready",
    );
    app.hub_mut().now_ms = 10_000;
    step(&mut ui, &mut app, &[AppMsg::TimerToggle], "6-timer-start");
    app.hub_mut().now_ms = 10_000 + 83_400;
    step(&mut ui, &mut app, &[AppMsg::TimerToggle], "7-timer-paused");
    step(&mut ui, &mut app, &[AppMsg::Gesture(SystemGesture::Up)], "8-home");
    assert_eq!(app.page(), Page::Home);
    assert_eq!(control.create_count(), 2, "the fox is remounted on return");

    ui.unmount();
    drop(ui);
    // SAFETY: the UI is gone; no object for this display remains.
    unsafe { display.delete() };
}

fn register_capturing_display(pixels: Capture) -> Display {
    Display::register::<_, { 480 * 4 }>(W, H, move |refresh| {
        let width = (refresh.area.x2 - refresh.area.x1 + 1) as usize;
        let height = (refresh.area.y2 - refresh.area.y1 + 1) as usize;
        let stride = W as usize * BPP;
        let mut pixels = pixels.borrow_mut();
        for y in 0..height {
            for x in 0..width {
                let color = refresh.colors[y * width + x];
                let i = (refresh.area.y1 as usize + y) * stride + (refresh.area.x1 as usize + x) * BPP;
                pixels[i] = color.r();
                pixels[i + 1] = color.g();
                pixels[i + 2] = color.b();
            }
        }
    })
    .expect("display")
}

fn refresh_full_screen() {
    // SAFETY: UI thread, outside `lv_timer_handler`; display and screen live.
    unsafe {
        lvgl_sys::lv_obj_invalidate(lvgl_sys::lv_screen_active());
        lvgl_sys::lv_refr_now(ptr::null_mut());
    }
}

fn write_ppm(path: &Path, pixels: &[u8]) {
    let mut file = fs::File::create(path).expect("create");
    write!(file, "P6\n{W} {H}\n255\n").expect("header");
    file.write_all(pixels).expect("pixels");
}
