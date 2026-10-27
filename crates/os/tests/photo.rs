//! Run the repository Photo app against real LVGL, native pointer input and
//! actual image decoders. Host libc files are mirrored into LittleFS for D: BIN.
use lua_engine::{Host, Limits, RawAllocator, Request, Step, Vm};
use os::lua_lvgl::LuaLvglService;
use rs_lvgl::{sys, Display};
use std::{
    alloc::Layout,
    cell::RefCell,
    ffi::CStr,
    fs,
    io::Write,
    path::Path,
    ptr,
    rc::Rc,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering},
    time::{Duration, UNIX_EPOCH},
};
unsafe extern "C" {
    static lv_label_class: sys::lv_obj_class_t;
    static lv_button_class: sys::lv_obj_class_t;
}
static NOW: AtomicU64 = AtomicU64::new(1_000_000);
static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);
static DOWN: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
fn clock_us() -> u64 {
    NOW.load(Ordering::Relaxed)
}
unsafe fn allocate(layout: Layout) -> *mut u8 {
    let p = unsafe { std::alloc::alloc(layout) };
    if !p.is_null() {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
    }
    p
}
unsafe fn deallocate(p: *mut u8, layout: Layout) {
    ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
    unsafe { std::alloc::dealloc(p, layout) };
}
static ALLOCATOR: RawAllocator = RawAllocator {
    alloc: allocate,
    dealloc: deallocate,
};
unsafe extern "C" fn input(_i: *mut sys::lv_indev_t, data: *mut sys::lv_indev_data_t) {
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
struct Console(Rc<RefCell<Vec<String>>>);
impl Host for Console {
    fn print(&mut self, line: &[u8]) {
        let line = String::from_utf8_lossy(line).into_owned();
        println!("LUA {line}");
        self.0.borrow_mut().push(line);
    }
}
struct Harness {
    vm: Option<Vm>,
    service: LuaLvglService,
    sleep_until: u64,
    finished: bool,
}
impl Harness {
    fn run(&mut self, ms: u32) {
        for _ in 0..ms.div_ceil(10) {
            NOW.fetch_add(10_000, Ordering::Relaxed);
            rs_lvgl::tick_inc(Duration::from_millis(10));
            unsafe {
                sys::lv_timer_handler();
            }
            if self.finished || clock_us() < self.sleep_until {
                continue;
            }
            let vm = self.vm.as_mut().unwrap();
            for _ in 0..10_000 {
                match vm.resume() {
                    Step::Slice => {}
                    Step::Request(Request::Host(lua_engine::ui::REQUEST)) => {
                        let command = vm.take_ui_command().expect("UI request owns its command");
                        let reply = self.service.execute(command);
                        vm.complete_ui(reply);
                    }
                    Step::Request(Request::Sleep { ms }) => {
                        self.sleep_until = clock_us() + ms as u64 * 1000;
                        break;
                    }
                    Step::Done => {
                        self.finished = true;
                        break;
                    }
                    other => panic!("album VM: {other:?}"),
                }
            }
        }
    }
    fn press(&mut self, x: i32, y: i32, ms: u32) {
        X.store(x, Ordering::Relaxed);
        Y.store(y, Ordering::Relaxed);
        DOWN.store(true, Ordering::Relaxed);
        self.run(ms);
    }
    fn release(&mut self) {
        DOWN.store(false, Ordering::Relaxed);
        self.run(150);
    }
    fn tap(&mut self, x: i32, y: i32) {
        self.press(x, y, 100);
        self.release();
    }
    fn label(&mut self, text: &str) {
        let point = unsafe {
            find_label(sys::lv_screen_active(), text)
                .map(|p| center(p))
                .unwrap_or_else(|| panic!("missing label {text}"))
        };
        self.tap(point.0, point.1);
    }
    fn first_tile(&mut self) {
        let point = unsafe {
            find_button(sys::lv_screen_active())
                .map(|p| center(p))
                .expect("grid button")
        };
        self.tap(point.0, point.1);
    }
}
unsafe fn center(p: *mut sys::lv_obj_t) -> (i32, i32) {
    unsafe {
        let mut a = core::mem::zeroed();
        sys::lv_obj_get_coords(p, &mut a);
        ((a.x1 + a.x2) / 2, (a.y1 + a.y2) / 2)
    }
}
unsafe fn find_label(p: *mut sys::lv_obj_t, text: &str) -> Option<*mut sys::lv_obj_t> {
    unsafe {
        if sys::lv_obj_check_type(p, &lv_label_class)
            && CStr::from_ptr(sys::lv_label_get_text(p)).to_bytes() == text.as_bytes()
        {
            return Some(p);
        }
        for i in 0..sys::lv_obj_get_child_count(p) {
            if let Some(found) = find_label(sys::lv_obj_get_child(p, i as i32), text) {
                return Some(found);
            }
        }
        None
    }
}
unsafe fn find_button(p: *mut sys::lv_obj_t) -> Option<*mut sys::lv_obj_t> {
    unsafe {
        if sys::lv_obj_check_type(p, &lv_button_class) {
            return Some(p);
        }
        for i in 0..sys::lv_obj_get_child_count(p) {
            if let Some(found) = find_button(sys::lv_obj_get_child(p, i as i32)) {
                return Some(found);
            }
        }
        None
    }
}
unsafe fn find_scroll(p: *mut sys::lv_obj_t) -> Option<*mut sys::lv_obj_t> {
    unsafe {
        if sys::lv_obj_get_scroll_bottom(p) > 50 {
            return Some(p);
        }
        for i in 0..sys::lv_obj_get_child_count(p) {
            if let Some(found) = find_scroll(sys::lv_obj_get_child(p, i as i32)) {
                return Some(found);
            }
        }
        None
    }
}
fn visible(text: &str) -> bool {
    unsafe { find_label(sys::lv_screen_active(), text).is_some() }
}
fn capture(out: &Path, name: &str, pixels: &Rc<RefCell<Vec<u8>>>) {
    unsafe {
        sys::lv_obj_invalidate(sys::lv_screen_active());
        sys::lv_refr_now(ptr::null_mut());
    }
    let mut file = fs::File::create(out.join(format!("{name}.ppm"))).unwrap();
    write!(file, "P6\n480 480\n255\n").unwrap();
    file.write_all(&pixels.borrow()).unwrap();
    println!("PASS {name}");
}
fn populate(root: &Path, fixtures: &Path) {
    fs::create_dir_all(root.join("photos")).unwrap();
    let littlefs = ::fs::Fs::mount_or_format(
        Box::new(::fs::RamDevice::new(4096, 256)),
        ::fs::Geometry::flash(256),
    )
    .unwrap()
    .0;
    let mut vfs = ::fs::Vfs::new();
    let mount = root
        .components()
        .nth(1)
        .unwrap()
        .as_os_str()
        .to_str()
        .unwrap();
    vfs.mount(mount, littlefs);
    vfs.create_dir_all(root.join("photos").to_str().unwrap())
        .unwrap();
    for n in 0..28 {
        let (name, source) = match n {
            0 => ("00-orientation.jpg".into(), "orientation.jpg"),
            1 => ("01-grayscale.jpeg".into(), "grayscale.jpeg"),
            2 => ("02-orientation.bin".into(), "orientation-rgb565.bin"),
            3 => ("03-corrupt.jpg".into(), "corrupt.jpg"),
            27 => ("ignored.png".into(), "pixels.png"),
            _ => (format!("{n:02}-orientation.jpg"), "orientation.jpg"),
        };
        let path = root.join("photos").join(name);
        let data = fs::read(fixtures.join(source)).unwrap();
        fs::write(&path, &data).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            )
            .unwrap();
        vfs.write_file(path.to_str().unwrap(), &data).unwrap();
    }
    ::fs::global::install(vfs);
}
#[test]
fn photo_navigation_deletion_and_cleanup() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = fs::read(workspace.join("apps/tools/photo/scripts/main.lua")).unwrap();
    let fixtures = fs::canonicalize(workspace.join("crates/lua-engine/test/images")).unwrap();
    let out = std::env::temp_dir().join(format!(
        "photo-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&out).unwrap();
    let out = fs::canonicalize(out).unwrap();
    let data = out.join("photo-data");
    populate(&data, &fixtures);
    unsafe {
        rs_lvgl::deinit();
    }
    rs_lvgl::init();
    let pixels = Rc::new(RefCell::new(vec![0u8; 480 * 480 * 3]));
    let flush = Rc::clone(&pixels);
    let display = Display::register::<_, { 480 * 48 }>(480, 480, move |region| {
        let width = (region.area.x2 - region.area.x1 + 1) as usize;
        let mut output = flush.borrow_mut();
        for y in 0..=(region.area.y2 - region.area.y1) as usize {
            for x in 0..width {
                let c = region.colors[y * width + x];
                let at = ((region.area.y1 as usize + y) * 480 + region.area.x1 as usize + x) * 3;
                output[at..at + 3].copy_from_slice(&[c.r(), c.g(), c.b()]);
            }
        }
    })
    .unwrap();
    let (previous, input_device) = unsafe {
        let previous = sys::lv_screen_active();
        let label = sys::lv_label_create(previous);
        sys::lv_label_set_text(label, c"Native home".as_ptr());
        let indev = sys::lv_indev_create();
        sys::lv_indev_set_type(indev, sys::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
        sys::lv_indev_set_read_cb(indev, Some(input));
        (previous, indev)
    };
    let logs = Rc::new(RefCell::new(Vec::new()));
    let mut vm = Vm::new(
        Box::new(Console(Rc::clone(&logs))),
        Limits {
            memory_bytes: 4 * 1024 * 1024,
            clock: Some(clock_us),
            ..Limits::default()
        },
        &ALLOCATOR,
        13,
    )
    .unwrap();
    vm.set_storage_root(data.to_str().unwrap()).unwrap();
    vm.set_args_json(r#"{"run_time_ms":1000000}"#).unwrap();
    vm.load("unchanged-factory-album", &source).unwrap();
    let mut h = Harness {
        vm: Some(vm),
        service: LuaLvglService::new(480, 480),
        sleep_until: 0,
        finished: false,
    };
    h.run(1800);
    assert!(visible("Album") && visible("27 photos"));
    assert!(logs
        .borrow()
        .iter()
        .any(|l| l.contains("found 27 image(s)")));
    capture(&out, "01-grid-native-images", &pixels);
    let scroll = unsafe { find_scroll(sys::lv_screen_active()).expect("scrollable grid") };
    h.press(400, 400, 100);
    for y in (160..400).rev().step_by(20) {
        h.press(400, y, 30);
    }
    h.release();
    assert!(
        unsafe { sys::lv_obj_get_scroll_y(scroll) } > 0,
        "real pointer drag scrolls grid"
    );
    capture(&out, "02-scrolled-grid", &pixels);
    unsafe {
        sys::lv_obj_scroll_to_y(scroll, 0, false);
    }
    h.run(100);
    h.first_tile();
    assert!(visible("00-orientation.jpg") && visible("1 / 27"));
    capture(&out, "03-jpeg-fullscreen", &pixels);
    h.label(">");
    assert!(visible("01-grayscale.jpeg") && visible("2 / 27"));
    capture(&out, "04-grayscale", &pixels);
    h.label(">");
    assert!(visible("02-orientation.bin") && visible("LVGL image"));
    capture(&out, "05-native-bin-decoder", &pixels);
    h.label(">");
    assert!(visible("03-corrupt.jpg") && visible("Image load failed"));
    capture(&out, "06-corrupt-image", &pixels);
    h.label("<");
    h.label("<");
    assert!(visible("01-grayscale.jpeg"));
    h.label("Delete");
    assert!(visible("Delete photo?"));
    capture(&out, "07-delete-confirm", &pixels);
    h.label("Cancel");
    assert!(!visible("Delete photo?") && data.join("photos/01-grayscale.jpeg").exists());
    h.label("Delete");
    // Two Delete labels now exist; the last/native dialog label is selected.
    let delete = unsafe {
        let mut labels = Vec::new();
        collect_labels(sys::lv_screen_active(), "Delete", &mut labels);
        center(*labels.last().unwrap())
    };
    h.tap(delete.0, delete.1);
    assert!(!data.join("photos/01-grayscale.jpeg").exists());
    assert!(visible("02-orientation.bin") && visible("2 / 26"));
    capture(&out, "08-deleted-and-advanced", &pixels);
    h.label("02-orientation.bin");
    assert!(visible("Album") && visible("26 photos"));
    let mut samples = Vec::new();
    for cycle in 0..48 {
        h.first_tile();
        assert!(visible("00-orientation.jpg"));
        h.label("00-orientation.jpg");
        h.run(60);
        assert!(visible("Album"));
        samples.push(h.vm.as_ref().unwrap().memory_used());
        assert!(h.service.object_count() < 160 && h.service.pixel_bytes() < 512 * 1024);
        if cycle == 47 {
            capture(&out, "09-after-48-view-cycles", &pixels);
        }
    }
    // Natural collection may produce a sawtooth; compare late low-water marks.
    let early = *samples[8..24].iter().min().unwrap();
    let late = *samples[32..48].iter().min().unwrap();
    assert!(
        late <= early + 128 * 1024,
        "retained callback growth: early={early} late={late}"
    );
    assert!(!logs
        .borrow()
        .iter()
        .any(|l| l.contains("lvgl callback failed")));
    h.vm.as_mut().unwrap().interrupt();
    // External app stop owns teardown; no Lua cleanup is required to make it safe.
    drop(h.vm.take());
    h.service.release();
    assert_eq!(h.service.object_count(), 0);
    assert_eq!(h.service.pixel_bytes(), 0);
    assert_eq!(ALLOCATED.load(Ordering::Relaxed), 0);
    assert_eq!(unsafe { sys::lv_screen_active() }, previous);
    assert!(visible("Native home"));
    capture(&out, "10-external-stop-restores-native", &pixels);
    // A button owns its implicit label. Cleaning children must invalidate that
    // label before set_text creates another, while stale explicit handles fail.
    let mut cleanup_vm = Vm::new(
        Box::new(Console(Rc::clone(&logs))),
        Limits {
            clock: Some(clock_us),
            ..Limits::default()
        },
        &ALLOCATOR,
        14,
    )
    .unwrap();
    cleanup_vm
        .load(
            "clean-text-lifetime",
            br#"
        local lvgl=require('lvgl')
        lvgl.init({})
        local screen=lvgl.create_screen()
        local button=lvgl.button(screen,{text='before clean',w=200,h=80})
        local child=lvgl.label(button,{text='old child'})
        button:clean()
        assert(not child:is_valid())
        assert(not pcall(child.set_text,child,'stale'))
        button:set_text('after clean')
        screen:load()
        require('delay').delay_ms(50)
    "#,
        )
        .unwrap();
    h.vm = Some(cleanup_vm);
    h.finished = false;
    h.sleep_until = 0;
    h.run(100);
    assert!(h.finished && visible("after clean") && !visible("Text"));
    capture(&out, "11-clean-then-set-text", &pixels);
    drop(h.vm.take());
    h.service.release();
    assert_eq!(ALLOCATED.load(Ordering::Relaxed), 0);
    assert_eq!(unsafe { sys::lv_screen_active() }, previous);
    fs::write(
        out.join("heap-samples.json"),
        serde_json::to_string_pretty(&samples).unwrap(),
    )
    .unwrap();
    fs::write(out.join("lua.log"), logs.borrow().join("\n")).unwrap();
    drop(h);
    unsafe {
        sys::lv_indev_delete(input_device);
    }
    drop(display);
    fs::remove_dir_all(out).unwrap();
}
unsafe fn collect_labels(p: *mut sys::lv_obj_t, wanted: &str, out: &mut Vec<*mut sys::lv_obj_t>) {
    unsafe {
        if sys::lv_obj_check_type(p, &lv_label_class)
            && CStr::from_ptr(sys::lv_label_get_text(p)).to_bytes() == wanted.as_bytes()
        {
            out.push(p);
        }
        for i in 0..sys::lv_obj_get_child_count(p) {
            collect_labels(sys::lv_obj_get_child(p, i as i32), wanted, out);
        }
    }
}
