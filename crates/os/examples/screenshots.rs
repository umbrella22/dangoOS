//! Export README screenshots from the production native UI and unchanged 2048 app.
//! Run from the repository root: cargo run -p os --example screenshots -- OUT
use lua_engine::{Blit, DisplayInfo, Host, Limits, Request, Step, TouchPoint, Vm};
use os::{
    AppMsg, Expression, FoxControl, MosaicoApp,
    fox::skin::Skin,
    hub::{
        AppId, Page,
        registry::{IconCache, Registry},
        settings,
    },
    ui::{splash::SplashControl, theme},
};
use rs_lvgl::{Display, sys};
use rs_lvgl_ui::{App, Ui};
use std::{
    cell::{Cell, RefCell},
    fs as host_fs,
    io::Write,
    path::Path,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

type Pixels = Rc<RefCell<Vec<u8>>>;
static NOW: AtomicU64 = AtomicU64::new(1_000_000);
fn clock() -> u64 {
    NOW.load(Ordering::Relaxed)
}

fn ppm(out: &Path, name: &str, pixels: &Pixels) {
    let mut file = host_fs::File::create(out.join(format!("{name}.ppm"))).unwrap();
    file.write_all(b"P6\n480 480\n255\n").unwrap();
    file.write_all(&pixels.borrow()).unwrap();
}
fn capture(out: &Path, name: &str, pixels: &Pixels) {
    unsafe {
        sys::lv_obj_invalidate(sys::lv_screen_active());
        sys::lv_refr_now(ptr::null_mut());
    }
    ppm(out, name, pixels);
}
fn run(ui: &mut Ui<AppMsg>, app: &mut MosaicoApp, ms: u32) {
    for _ in 0..ms.div_ceil(50) {
        rs_lvgl::tick_inc(Duration::from_millis(50));
        ui.tick(50).unwrap();
        ui.timer_handler();
        ui.drain(app, 32).unwrap();
    }
}
fn copy_tree(vfs: &mut fs::Vfs, source: &Path, destination: &str) {
    vfs.create_dir_all(destination).unwrap();
    for entry in host_fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        if name.starts_with('.') {
            continue;
        }
        let target = format!("{destination}/{name}");
        if entry.file_type().unwrap().is_dir() {
            copy_tree(vfs, &entry.path(), &target);
        } else {
            vfs.write_file(&target, &host_fs::read(entry.path()).unwrap())
                .unwrap();
        }
    }
}
fn registry(root: &Path) -> Registry {
    let volume = fs::Fs::mount_or_format(
        Box::new(fs::RamDevice::new(4096, 1024)),
        fs::Geometry::flash(1024),
    )
    .unwrap()
    .0;
    let mut vfs = fs::Vfs::new();
    vfs.mount("system", volume);
    for category in ["games", "tools"] {
        for directory in host_fs::read_dir(root.join("apps").join(category)).unwrap() {
            let directory = directory.unwrap();
            if directory.path().join("launcher.json").is_file() {
                copy_tree(
                    &mut vfs,
                    &directory.path(),
                    &format!("/system/apps/{}", directory.file_name().to_str().unwrap()),
                );
            }
        }
    }
    Registry::scan(&mut vfs, &mut IconCache::default(), &mut |line| {
        println!("{line}")
    })
}

fn skin_choices(root: &Path) -> Vec<os::hub::skins::Choice> {
    let volume = fs::Fs::mount_or_format(
        Box::new(fs::RamDevice::new(4096, 32)),
        fs::Geometry::flash(32),
    )
    .unwrap()
    .0;
    let mut vfs = fs::Vfs::new();
    vfs.mount("data", volume);
    for id in ["eclipse", "normal", "black-blue"] {
        let directory = format!("/data/skins/{id}");
        vfs.create_dir_all(&directory).unwrap();
        let metadata = host_fs::read(root.join("dango-skin").join(id).join("skin.json")).unwrap();
        vfs.write_file(&format!("{directory}/skin.json"), &metadata)
            .unwrap();
    }
    os::hub::skins::choices(&mut vfs).unwrap()
}

struct GameHost {
    pixels: Pixels,
    contact: Rc<Cell<Option<TouchPoint>>>,
}
impl Host for GameHost {
    fn print(&mut self, line: &[u8]) {
        println!("2048: {}", String::from_utf8_lossy(line));
    }
    fn unix_time_us(&self) -> Option<i64> {
        Some(1_700_000_000_000_000)
    }
    fn display_open(&mut self) -> Result<DisplayInfo, i32> {
        Ok(self.display_info().unwrap())
    }
    fn display_info(&self) -> Option<DisplayInfo> {
        Some(DisplayInfo {
            width: 480,
            height: 480,
            touch_available: true,
        })
    }
    fn display_blit(&mut self, blit: &Blit) -> Result<(), i32> {
        let mut pixels = self.pixels.borrow_mut();
        for y in 0..blit.height() {
            for (x, color) in blit.row(y).enumerate() {
                let at = ((y + blit.y0 as usize) * 480 + x + blit.x0 as usize) * 3;
                pixels[at..at + 3].copy_from_slice(&[
                    ((color >> 11) * 255 / 31) as u8,
                    (((color >> 5) & 63) * 255 / 63) as u8,
                    ((color & 31) * 255 / 31) as u8,
                ]);
            }
        }
        Ok(())
    }
    fn touch(&mut self, out: &mut [TouchPoint]) -> Result<usize, i32> {
        if let Some(point) = self.contact.get() {
            out[0] = point;
            Ok(1)
        } else {
            Ok(0)
        }
    }
}
fn play(vm: &mut Vm, ms: u32) {
    let until = clock() + u64::from(ms) * 1000;
    while clock() < until {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(Request::Present) => {
                NOW.fetch_add(16_000, Ordering::Relaxed);
            }
            Step::Request(Request::Sleep { ms }) => {
                NOW.fetch_add(u64::from(ms.max(1)) * 1000, Ordering::Relaxed);
            }
            other => panic!("2048 stopped: {other:?}"),
        }
    }
}
fn game(root: &Path, out: &Path) {
    let pixels = Rc::new(RefCell::new(vec![0; 480 * 480 * 3]));
    let contact = Rc::new(Cell::new(None));
    let mut vm = Vm::new(
        Box::new(GameHost {
            pixels: pixels.clone(),
            contact: contact.clone(),
        }),
        Limits {
            clock: Some(clock),
            ..Limits::default()
        },
        &lua_engine::GLOBAL_ALLOCATOR,
        1,
    )
    .unwrap();
    vm.load(
        "2048",
        &host_fs::read(root.join("apps/games/2048/scripts/main.lua")).unwrap(),
    )
    .unwrap();
    play(&mut vm, 200);
    // Feed actual swipe contacts through the app's display:touch() API.
    for index in 0..28 {
        let (start, end) = match index % 4 {
            0 => ((350, 260), (120, 260)),
            1 => ((240, 350), (240, 130)),
            2 => ((120, 260), (350, 260)),
            _ => ((240, 350), (240, 130)),
        };
        for point in [start, end] {
            contact.set(Some(TouchPoint {
                id: 1,
                x: point.0,
                y: point.1,
            }));
            play(&mut vm, 80);
        }
        contact.set(None);
        play(&mut vm, 120);
    }
    ppm(out, "2048", &pixels);
}
fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::env::args_os()
        .nth(1)
        .expect("output directory required");
    let out = Path::new(&out);
    host_fs::create_dir_all(out).unwrap();
    rs_lvgl::init();
    let pixels = Rc::new(RefCell::new(vec![0; 480 * 480 * 3]));
    let output = pixels.clone();
    let display = Display::register::<_, { 480 * 48 }>(480, 480, move |r| {
        let width = (r.area.x2 - r.area.x1 + 1) as usize;
        let height = (r.area.y2 - r.area.y1 + 1) as usize;
        let mut pixels = output.borrow_mut();
        for y in 0..height {
            for x in 0..width {
                let c = r.colors[y * width + x];
                let at = ((y + r.area.y1 as usize) * 480 + x + r.area.x1 as usize) * 3;
                pixels[at..at + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
            }
        }
    })
    .unwrap();
    let control = FoxControl::new(Expression::Idle);
    let mut app = MosaicoApp::new(control.clone(), SplashControl::new());
    app.enter_fox();
    app.set_registry(registry(&root));
    let mut ui = Ui::new(&display, theme()).unwrap();
    ui.mount(&app).unwrap();
    for id in ["default", "eclipse", "normal", "black-blue"] {
        control.set_skin(if id == "default" {
            Skin::builtin()
        } else {
            Skin::parse(host_fs::read(root.join("dango-skin").join(id).join("fox.bin")).unwrap())
                .unwrap()
        });
        run(&mut ui, &mut app, 2500);
        capture(out, id, &pixels);
    }
    app.go(Page::Launcher);
    ui.mark_dirty();
    run(&mut ui, &mut app, 200);
    capture(out, "launcher", &pixels);
    app.go(Page::App(AppId::Settings));
    app.settings_session_mut().info.storage_ready = true;
    app.settings_session_mut().skins = skin_choices(&root);
    app.settings_session_mut().selected_skin = "black-blue".into();
    ui.mark_dirty();
    run(&mut ui, &mut app, 200);
    capture(out, "settings", &pixels);
    // Exercise the longest bundled name in the root row as well as the chooser.
    app.settings_session_mut().selected_skin = "default".into();
    ui.mark_dirty();
    run(&mut ui, &mut app, 200);
    capture(out, "settings-default", &pixels);
    app.settings_session_mut().selected_skin = "black-blue".into();
    app.update(AppMsg::SettingsPage(settings::Page::Character));
    ui.mark_dirty();
    run(&mut ui, &mut app, 200);
    capture(out, "characters", &pixels);
    ui.unmount();
    drop(ui);
    unsafe {
        display.delete();
        rs_lvgl::deinit();
    }
    game(&root, out);
}
