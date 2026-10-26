//! Host tests for the esp-claw compatible modules: the official display
//! contract, delay/system/os/json/arg_schema, and the Snake and 2048 apps
//! from esp-claw-skills-lab running unchanged.

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::cell::Cell;
use std::sync::Mutex;

use crate::*;

/// The display module keeps C globals (one screen per device), so tests that
/// open it run one at a time.
static DISPLAY_LOCK: Mutex<()> = Mutex::new(());

std::thread_local! {
    static NOW: Cell<u64> = const { Cell::new(1_000_000) };
}

/// Simulated time: 5 us per reading, plus whatever the test loop sleeps.
fn sim_clock() -> u64 {
    NOW.with(|n| {
        let t = n.get() + 5;
        n.set(t);
        t
    })
}

fn advance_ms(ms: u32) {
    NOW.with(|n| n.set(n.get() + ms as u64 * 1000));
}

const W: u16 = 480;
const H: u16 = 480;

struct Screen {
    lines: Vec<String>,
    logs: Vec<String>,
    open: bool,
    opens: u32,
    frame: Vec<u16>,
    blits: u32,
    presents: u32,
    touch_script: Vec<Option<(i32, i32)>>,
    touch_reads: usize,
}

impl Screen {
    fn new() -> Screen {
        Screen {
            lines: Vec::new(),
            logs: Vec::new(),
            open: false,
            opens: 0,
            frame: vec![0; W as usize * H as usize],
            blits: 0,
            presents: 0,
            touch_script: Vec::new(),
            touch_reads: 0,
        }
    }
}

impl Host for Screen {
    fn print(&mut self, line: &[u8]) {
        self.lines.push(String::from_utf8_lossy(line).into_owned());
    }
    fn log(&mut self, line: &[u8]) {
        self.logs.push(String::from_utf8_lossy(line).into_owned());
    }
    fn display_open(&mut self) -> Result<DisplayInfo, i32> {
        if self.open {
            return Err(esp_err::INVALID_STATE);
        }
        self.open = true;
        self.opens += 1;
        Ok(self.display_info().unwrap())
    }
    fn display_info(&self) -> Option<DisplayInfo> {
        self.open.then_some(DisplayInfo {
            width: W,
            height: H,
            touch_available: true,
        })
    }
    fn display_close(&mut self) {
        self.open = false;
    }
    fn display_blit(&mut self, blit: &Blit) -> Result<(), i32> {
        for r in 0..blit.height() {
            let y = blit.y0 as usize + r;
            for (i, px) in blit.row(r).enumerate() {
                self.frame[y * W as usize + blit.x0 as usize + i] = px;
            }
        }
        self.blits += 1;
        Ok(())
    }
    fn touch(&mut self, out: &mut [TouchPoint]) -> Result<usize, i32> {
        let i = self.touch_reads;
        self.touch_reads += 1;
        match self.touch_script.get(i).copied().flatten() {
            Some((x, y)) => {
                out[0] = TouchPoint { id: 0, x, y };
                Ok(1)
            }
            None => Ok(0),
        }
    }
    fn heap_info(&mut self, caps: u32) -> HeapInfo {
        let total = 8 * 1024 * 1024u32;
        let used = platform_bytes() as u32;
        HeapInfo {
            total_size: total,
            free_size: total - used,
            allocated_size: used,
            largest_free_block: total - used,
            ..HeapInfo::default()
        }
        .with_caps(caps)
    }
}

trait WithCaps {
    fn with_caps(self, caps: u32) -> Self;
}
impl WithCaps for HeapInfo {
    fn with_caps(self, _caps: u32) -> Self {
        self
    }
}

fn limits() -> Limits {
    Limits {
        memory_bytes: 4 * 1024 * 1024,
        hook_interval: 200,
        clock: Some(sim_clock),
        ..Limits::default()
    }
}

fn vm() -> Vm {
    Vm::new(Box::new(Screen::new()), limits(), &GLOBAL_ALLOCATOR, 7).unwrap()
}

struct Outcome {
    step: Step,
    sleeps: Vec<u32>,
    presents: u32,
}

/// Runs until the script ends or `max_presents` frames were shown.
fn drive(vm: &mut Vm, max_presents: u32) -> Outcome {
    drive_for(vm, max_presents, usize::MAX)
}

/// Like [`drive`], also stopping after `max_sleeps` sleep requests.
fn drive_for(vm: &mut Vm, max_presents: u32, max_sleeps: usize) -> Outcome {
    let mut sleeps = Vec::new();
    let mut presents = 0;
    for _ in 0..2_000_000 {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(Request::Sleep { ms }) => {
                sleeps.push(ms);
                advance_ms(ms);
                if sleeps.len() >= max_sleeps {
                    vm.interrupt();
                }
            }
            Step::Request(Request::Present) => {
                presents += 1;
                vm.host_mut::<Screen>().unwrap().presents += 1;
                if presents >= max_presents {
                    vm.interrupt();
                }
            }
            Step::Request(other) => panic!("unexpected request {other:?}"),
            step => {
                return Outcome {
                    step,
                    sleeps,
                    presents,
                };
            }
        }
    }
    panic!("script did not finish");
}

fn run_src(vm: &mut Vm, src: &str) -> Outcome {
    vm.load("app.lua", src.as_bytes()).unwrap();
    drive(vm, u32::MAX)
}

fn screen(vm: &mut Vm) -> &mut Screen {
    vm.host_mut::<Screen>().unwrap()
}

#[test]
fn display_contract_passes() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = platform_bytes();
    let mut vm = vm();
    vm.set_args_json(r#"{"cycles": 20}"#).unwrap();
    let src = include_str!("../vendor/esp-claw/lua_module_display/test/display_contract.lua");
    vm.load("display_contract.lua", src.as_bytes()).unwrap();
    let out = drive(&mut vm, u32::MAX);
    let s = screen(&mut vm);
    assert_eq!(out.step, Step::Done, "{:?} {:?}", s.lines, s.logs);
    assert!(
        s.lines
            .iter()
            .any(|l| l.starts_with("[display_contract] buffers=1")),
        "{:?}",
        s.lines
    );
    assert!(
        s.lines
            .iter()
            .any(|l| l.starts_with("[display_contract] buffers=2"))
    );
    let pass = s
        .lines
        .iter()
        .find(|l| l.contains("PASS cycles=20"))
        .expect("PASS line");
    assert!(pass.ends_with("delta=0"), "{pass}");
    assert!(!s.open);
    assert!(out.presents > 20);
    drop(vm);
    assert_eq!(platform_bytes(), before);
}

#[test]
fn display_pixels_reach_the_host() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut vm = vm();
    let out = run_src(
        &mut vm,
        r##"
        local display = require("display")
        local s <close> = display.open()
        s:begin({ clear = 0xFF000000 })
        s:fill_rect(10, 20, 4, 2, "#ff0000")
        s:text(100, 100, "Hi", { color = 0xFFFFFFFF, font_size = 24 })
        assert(s:present() == true)
        "##,
    );
    assert_eq!(out.step, Step::Done, "{:?}", screen(&mut vm).logs);
    assert_eq!(out.presents, 1);
    let s = screen(&mut vm);
    assert_eq!(s.frame[20 * 480 + 10], 0xF800);
    assert_eq!(s.frame[21 * 480 + 13], 0xF800);
    assert_eq!(s.frame[22 * 480 + 10], 0);
    let lit = (100..124)
        .flat_map(|y| (100..130).map(move |x| (x, y)))
        .filter(|&(x, y)| s.frame[y * 480 + x] != 0)
        .count();
    assert!(lit > 20, "text drew {lit} pixels");
    assert!(!s.open, "<close> released the screen");
}

#[test]
fn display_is_released_when_the_vm_is_dropped() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = platform_bytes();
    let mut vm = vm();
    vm.load(
        "app.lua",
        br#"
        local s = require("display").open()
        s:begin()
        require("delay").delay_ms(1000)
    "#,
    )
    .unwrap();
    assert_eq!(vm.resume(), Step::Request(Request::Sleep { ms: 1000 }));
    assert!(screen(&mut vm).open);
    assert!(platform_bytes() > before);
    drop(vm);
    assert_eq!(platform_bytes(), before);
    // A new script can open it again.
    let mut vm2 = self::vm();
    assert_eq!(
        run_src(&mut vm2, "require('display').open():close()").step,
        Step::Done
    );
}

#[test]
fn delay_suspends_the_script() {
    let mut vm = vm();
    let out = run_src(
        &mut vm,
        r#"
        local delay = require("delay")
        delay.delay_ms(250)
        delay.delay_ms(-5)
        delay.delay_us(10)
        assert(not pcall(delay.delay_us, 2000000))
        local t = delay.periodic(100)
        assert(t:wait() == true)
        assert(t:wait() == true)
        assert(not pcall(delay.periodic, 0))
        -- inside a coroutine the delay cannot yield the script: it blocks
        local co = coroutine.wrap(function() delay.delay_ms(3) return "done" end)
        assert(co() == "done")
        print("ok")
        "#,
    );
    assert_eq!(out.step, Step::Done, "{:?}", screen(&mut vm).lines);
    assert_eq!(out.sleeps.len(), 3, "{:?}", out.sleeps);
    assert_eq!(out.sleeps[0], 250);
    assert!(out.sleeps[1] >= 99 && out.sleeps[1] <= 100);
    assert_eq!(screen(&mut vm).lines, ["ok"]);
}

#[test]
fn system_os_json_and_arg_schema() {
    let mut vm = vm();
    vm.set_args_json(r#"{"speed": 3, "name": "fox", "nested": {"on": true}}"#)
        .unwrap();
    let out = run_src(
        &mut vm,
        r#"
        local system = require("system")
        local json = require("json")
        local arg_schema = require("arg_schema")
        assert(type(system.millis()) == "number" and system.millis() >= 1000)
        assert(math.type(system.uptime()) == "integer")
        assert(type(system.time()) == "number")
        assert(#system.date() == 19, system.date())
        assert(system.date("%Y") == "1970")
        assert(system.ip() == nil)
        assert(system.heap.get_info(system.heap.caps.DEFAULT).total_size > 0)
        assert(type(system.info().sram_free) == "number")
        assert(type(os.time()) == "number" and type(os.clock()) == "number")
        assert(os.date("!%Y-%m-%d", 0) == "1970-01-01")
        assert(os.time({year=2026, month=10, day=22, hour=12}) ~= nil)
        assert(os.getenv("HOME") == nil or true)
        assert(os.execute() == false)
        assert(os.remove("x") == nil)
        assert(args.speed == 3 and args.name == "fox" and args.nested.on == true)
        local s = json.encode({a = 1, b = {true, false}, c = "x"})
        local t = json.decode(s)
        assert(t.a == 1 and t.b[1] == true and t.b[2] == false and t.c == "x")
        assert(not pcall(json.decode, "{oops"))
        local ctx = arg_schema.parse(args, {
            speed = arg_schema.int({ default = 1, min = 0, max = 5 }),
            nested = arg_schema.object({ fields = { on = arg_schema.bool({ default = false }) } }),
            missing = arg_schema.bool({ default = true }),
        })
        assert(ctx.speed == 3 and ctx.nested.on == true and ctx.missing == true)
        print("ok")
        "#,
    );
    assert_eq!(out.step, Step::Done, "{:?}", screen(&mut vm).lines);
}

fn app(path: &str) -> &'static str {
    match path {
        "snake" => include_str!("../vendor/skills-lab/snake/scripts/draw_snake.lua"),
        "game_2048" => include_str!("../vendor/skills-lab/game_2048/scripts/game_2048.lua"),
        _ => unreachable!(),
    }
}

#[test]
fn snake_runs_unchanged() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut vm = vm();
    vm.load("draw_snake.lua", app("snake").as_bytes()).unwrap();
    let out = drive(&mut vm, 60);
    let s = screen(&mut vm);
    assert_eq!(out.step, Step::Interrupted, "{:?} {:?}", s.lines, s.logs);
    assert_eq!(out.presents, 60);
    assert!(!out.sleeps.is_empty());
    let colours: std::collections::BTreeSet<u16> = s.frame.iter().copied().collect();
    assert!(colours.len() >= 3, "frame has {} colours", colours.len());
    drop(vm);
}

#[test]
fn game_2048_runs_and_takes_swipes() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut vm = vm();
    // A few idle polls, then a long swipe left and release, repeated.
    let mut script = Vec::new();
    for _ in 0..4 {
        script.extend([
            None,
            None,
            Some((400, 240)),
            Some((300, 240)),
            Some((60, 240)),
            None,
        ]);
        script.extend([
            None,
            Some((240, 60)),
            Some((240, 300)),
            Some((240, 420)),
            None,
        ]);
    }
    screen(&mut vm).touch_script = script;
    vm.load("game_2048.lua", app("game_2048").as_bytes())
        .unwrap();
    let out = drive_for(&mut vm, u32::MAX, 400);
    let s = screen(&mut vm);
    assert_eq!(out.step, Step::Interrupted, "{:?} {:?}", s.lines, s.logs);
    assert!(s.touch_reads > 20, "touch polled {} times", s.touch_reads);
    // The first frame plus redraws after the swipes.
    assert!(out.presents >= 3, "{} frames", out.presents);
    assert!(
        s.lines.iter().any(|l| l.contains("audio unavailable")),
        "{:?}",
        s.lines
    );
    assert!(
        !s.lines.iter().any(|l| l.contains("ERROR")),
        "{:?}",
        s.lines
    );
}

/// A fresh directory under the system temp dir.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(alloc::format!("mosaico-lua-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn file_apps_require_io_and_storage() {
    // The data root is global; tests that set it take the same lock.
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_dir("files");
    let app = root.join("apps/demo");
    std::fs::create_dir_all(app.join("scripts/lib")).unwrap();
    let shared = root.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        app.join("scripts/helper.lua"),
        "return { twice = function(x) return 2 * x end }",
    )
    .unwrap();
    std::fs::write(app.join("scripts/lib/init.lua"), "return 'lib-init'").unwrap();
    std::fs::write(shared.join("common.lua"), "return 'shared'").unwrap();
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        app.join("scripts/main.lua"),
        r#"
        local h = require("helper")
        assert(h.twice(21) == 42)
        assert(require("lib") == "lib-init")
        assert(require("common") == "shared")
        local storage = require("storage")
        local root = storage.get_root_dir()
        local p = storage.join_path(root, "notes")
        storage.mkdir(p)
        storage.write_file(p .. "/a.txt", "hello")
        assert(storage.read_file(p .. "/a.txt") == "hello")
        assert(storage.exists(p .. "/a.txt"))
        local f = assert(io.open(p .. "/b.txt", "w"))
        f:write("line1\n", 2, "\n")
        f:close()
        local lines = {}
        for l in io.lines(p .. "/b.txt") do lines[#lines + 1] = l end
        assert(#lines == 2 and lines[2] == "2", table.concat(lines, ","))
        assert(os.rename(p .. "/b.txt", p .. "/c.txt"))
        assert(os.remove(p .. "/c.txt"))
        local names = {}
        for _, e in ipairs(storage.listdir(p)) do names[#names + 1] = type(e) == "table" and e.name or e end
        assert(#names == 1, #names)
        assert(storage.remove_tree(p))
        assert(not storage.exists(p))
        assert(dofile ~= nil and loadfile ~= nil)
        assert(not pcall(require, "no_such_module"))
        print("ok")
        "#,
    )
    .unwrap();
    let mut vm = vm();
    vm.set_storage_root(data.to_str().unwrap()).unwrap();
    let entry = app.join("scripts/main.lua");
    vm.load_file(entry.to_str().unwrap(), &[shared.to_str().unwrap()])
        .unwrap();
    let out = drive(&mut vm, u32::MAX);
    let s = screen(&mut vm);
    assert_eq!(out.step, Step::Done, "{:?} {:?}", s.lines, s.logs);
    assert_eq!(s.lines, ["ok"]);
    drop(vm);
    let mut vm = vm_fresh();
    let err = vm
        .load_file(root.join("missing.lua").to_str().unwrap(), &[])
        .unwrap_err();
    assert!(err.contains("missing.lua"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

fn vm_fresh() -> Vm {
    vm()
}

/// The factory apps from the device's `/system` partition, when extracted
/// to `MOSAICO_SYSFS` (they are not redistributed with this repository).
#[test]
fn example_push_app_runs() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let app =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/lua-apps/hello_push");
    let data = temp_dir("hello_push");
    for launch in 1..=2u32 {
        let mut vm = vm();
        vm.set_storage_root(data.to_str().unwrap()).unwrap();
        vm.load_file(app.join("scripts/main.lua").to_str().unwrap(), &[])
            .unwrap();
        let out = drive(&mut vm, 10);
        let s = screen(&mut vm);
        assert_eq!(out.step, Step::Interrupted, "{:?} {:?}", s.lines, s.logs);
        drop(vm);
        let count = std::fs::read_to_string(data.join("hello_push.count")).unwrap();
        assert_eq!(count.trim(), alloc::format!("{launch}"));
    }
}

#[test]
fn official_system_apps_run() {
    let Some(sysfs) = std::env::var_os("MOSAICO_SYSFS") else {
        return;
    };
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sysfs = std::path::PathBuf::from(sysfs);
    for (id, entry) in [
        ("dino", "scripts/dino.lua"),
        ("flappybird", "scripts/flappybird.lua"),
    ] {
        let path = sysfs.join("apps").join(id).join(entry);
        if !path.exists() {
            continue;
        }
        let mut vm = vm();
        let builtin = sysfs.join("scripts/builtin");
        vm.load_file(path.to_str().unwrap(), &[builtin.to_str().unwrap()])
            .unwrap();
        let out = drive(&mut vm, 40);
        let s = screen(&mut vm);
        assert_eq!(
            out.step,
            Step::Interrupted,
            "{id}: {:?} {:?}",
            s.lines,
            s.logs
        );
        drop(vm);
    }
}

#[test]
fn hardware_probe_draws_unavailable_hardware_without_output_side_effects() {
    let _g = DISPLAY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut vm = vm();
    vm.set_args_json(r#"{"outputs":false,"duration_ms":100}"#)
        .unwrap();
    let source = include_str!("../../../examples/lua-apps/hardware_probe/scripts/main.lua");
    let out = run_src(&mut vm, source);
    assert_eq!(out.step, Step::Done);
    assert_eq!(out.presents, 2);
    let s = screen(&mut vm);
    assert!(
        s.lines
            .iter()
            .any(|line| line.contains("hardware unavailable"))
    );
    assert!(s.frame.contains(&0x07e0), "opaque green text was drawn");
    assert!(s.frame.contains(&0xffff), "opaque white title was drawn");
    assert!(!s.open, "example closes display");
}

#[test]
fn image_png_jpeg_frames_resize_release_and_caps() {
    let mut vm = vm();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/n5-images");
    let source = alloc::format!(
        r#"
        local image=require('image')
        local dir='{}/'
        local frame <close> = image.load_file(dir..'pixels.png')
        local info=frame:info();assert(info.width==3 and info.height==2 and info.pixel_format=='RGB3')
        assert(frame:data()==string.char(255,0,0,0,255,0,0,0,255,255,255,255,0,0,0,255,255,0))
        local tiny <close> = image.resize(frame,{{width=3,height=2,filter='nearest'}})
        assert(tiny:data()==string.char(0,248,224,7,31,0,255,255,0,0,224,255))
        frame:release();assert(not frame:info().valid)
        assert(not pcall(frame.data,frame));assert(#tiny:data()==12)
        local alpha <close> = image.load_file(dir..'alpha.png')
        assert(alpha:data()==string.char(255,0,0,0,128,0,0,0,0,64,64,64,0,0,0,192,192,0))
        for _,name in ipairs({{'orientation.jpg','grayscale.jpeg'}}) do
            local jpg <close> = image.load_file(dir..name)
            assert(jpg:info().pixel_format=='JPEG')
            local small <close> = image.resize(jpg,{{width=8,height=8,filter='bilinear'}})
            assert(small:info().pixel_format=='RGBP' and #small:data()==128)
            local gray <close> = image.convert(small,image.GRAY8)
            assert(#gray:data()==64)
            local encoded <close> = image.convert(small,image.JPEG)
            assert(encoded:info().pixel_format=='JPEG' and encoded:data():sub(1,2)==string.char(255,216))
            local roundtrip <close> = image.convert(encoded,image.RGB888)
            assert(#roundtrip:data()==8*8*3)
            assert(not pcall(image.convert,small,4294967296))
            assert(not pcall(image.resize,small,{{width=8,height=8,format=4294967296}}))
            assert(not pcall(image.save_file,dir..'unsupported.png',encoded))
        end
        for _,name in ipairs({{'truncated.png','truncated.jpg','corrupt.jpg'}}) do
            assert(not pcall(function()
                local bad <close> = image.load_file(dir..name)
                local decoded <close> = image.resize(bad,{{width=8,height=8}})
            end))
        end
        assert(not pcall(image.resize,tiny,{{width=65535,height=65535}}))
        collectgarbage('collect')
    "#,
        dir.display()
    );
    assert_eq!(run_src(&mut vm, &source).step, Step::Done);
    assert!(vm.memory_used() < limits().memory_bytes);
}

#[test]
fn identity_bilinear_resize_matches_conversion_and_nearest() {
    let mut vm = vm();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/n5-images");
    let source = alloc::format!(
        r#"
        local image=require('image')
        for _,name in ipairs({{'pixels.png','alpha.png','orientation.jpg','grayscale.jpeg'}}) do
            local original <close> = image.load_file('{}/'..name)
            local rgb565 <close> = image.convert(original,image.RGB565)
            local gray <close> = image.convert(original,image.GRAY8)
            for _,frame in ipairs({{original,rgb565,gray}}) do
                local info=frame:info()
                for _,format in ipairs({{image.RGB565,image.GRAY8}}) do
                    local expected <close> = image.convert(frame,format)
                    local bilinear <close> = image.resize(frame,{{width=info.width,height=info.height,format=format,filter='bilinear'}})
                    local nearest <close> = image.resize(frame,{{width=info.width,height=info.height,format=format,filter='nearest'}})
                    assert(bilinear:data()==expected:data(),name..' identity differs from format conversion')
                    assert(bilinear:data()==nearest:data(),name..' identity differs from nearest')
                    assert(bilinear:info().width==info.width and bilinear:info().height==info.height)
                end
            end
        end
        "#,
        dir.display()
    );
    assert_eq!(run_src(&mut vm, &source).step, Step::Done);
}

#[test]
fn tiled_bilinear_matches_original_c_formula() {
    // Compile the frozen pre-optimization expression with the same host C
    // compiler and -O2 as build.rs. A Lua arithmetic oracle is insufficient:
    // C may contract multiply/add operations inside its interpolation expression.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("testdata/n5-images");
    let scratch = std::env::temp_dir().join(alloc::format!("n5-resize-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let records = scratch.join("cases.bin");
    let executable = scratch.join("original-resize-oracle");
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let compiled = std::process::Command::new(compiler)
        .args(["-std=gnu11", "-O2"])
        .arg(root.join("testdata/n5-resize-oracle.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut vm = vm();
    let source = alloc::format!(
        r#"
local image=require('image')
local output <close> = assert(io.open('{}/cases.bin','wb'))
for _,name in ipairs({{'pixels.png','alpha.png','orientation.jpg','grayscale.jpeg'}}) do
 local original <close> = image.load_file('{}/'..name)
 local packed <close> = image.convert(original,image.RGB565)
 local gray <close> = image.convert(original,image.GRAY8)
 local album <close> = image.resize(original,{{width=160,height=96,filter='nearest'}})
 for _,frame in ipairs({{original,packed,gray,album}}) do
  local rgb <close> = image.convert(frame,image.RGB888)
  local data=rgb:data();local info=rgb:info()
  for _,size in ipairs({{{{1,1}},{{2,3}},{{7,5}},{{31,9}},{{32,11}},{{33,7}},{{65,13}},{{80,48}},{{81,48}},{{117,73}},{{160,96}}}}) do
   local w,h=size[1],size[2]
   local actual <close> = image.resize(frame,{{width=w,height=h,filter='bilinear',format=image.RGB565}})
   local actualgray <close> = image.resize(frame,{{width=w,height=h,filter='bilinear',format=image.GRAY8}})
   assert(output:write(string.pack('<I4I4I4I4',info.width,info.height,w,h),data,actual:data(),actualgray:data()))
  end
 end
end
"#,
        scratch.display(),
        dir.display()
    );
    assert_eq!(run_src(&mut vm, &source).step, Step::Done);
    let compared = std::process::Command::new(executable)
        .arg(records)
        .output()
        .unwrap();
    assert!(
        compared.status.success(),
        "{}",
        String::from_utf8_lossy(&compared.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&compared.stdout).trim(),
        "176 cases: exact RGB565 and GRAY8"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn integer_bilinear_counterexample_search() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let executable = std::env::temp_dir().join(alloc::format!(
        "n5-integer-resize-stress-{}",
        std::process::id()
    ));
    let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let compiled = std::process::Command::new(compiler)
        .args(["-std=gnu11", "-O2"])
        .arg(root.join("testdata/n5-resize-stress.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let compared = std::process::Command::new(&executable).output().unwrap();
    assert!(
        compared.status.success(),
        "{}",
        String::from_utf8_lossy(&compared.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&compared.stdout).trim(),
        "10052 cases, 48881924 output pixels per format: exact RGB565 and GRAY8"
    );
    std::fs::remove_file(executable).unwrap();
}

#[test]
fn independent_workers_upstream_thread_contract() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/n5-thread");
    for case in [
        "api",
        "run_args",
        "full_flow",
        "job_named_replace",
        "job_exclusive",
        "queue_semantics",
        "semaphore_counting",
        "lock_contention",
        "stop_queue",
        "stop_lock",
        "invalid_args_opts",
        "sync_errors",
        "stability_cycles",
        "concurrency",
    ] {
        let mut vm = vm();
        vm.set_args_json(
            &serde_json::json!({"base":base.to_str().unwrap(),"case":case}).to_string(),
        )
        .unwrap();
        vm.load_file(base.join("harness.lua").to_str().unwrap(), &[])
            .unwrap();
        let mut done = false;
        for _ in 0..200_000 {
            match vm.resume() {
                Step::Slice => {}
                Step::Request(Request::Sleep { ms }) => {
                    for _ in 0..ms {
                        advance_ms(1);
                        vm.poll_workers();
                    }
                }
                Step::Done => {
                    done = true;
                    break;
                }
                e => panic!(
                    "thread case {case}: {e:?}; logs {:?}",
                    screen(&mut vm).lines
                ),
            }
        }
        assert!(done, "case {case} did not finish");
        assert!(
            screen(&mut vm)
                .lines
                .iter()
                .any(|s| s == &alloc::format!("case {case} ok")),
            "missing completion for {case}"
        );
    }
}

#[test]
fn workers_are_isolated_bounded_and_parent_interrupt_releases_group() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/n5-thread");
    let path = base.join("hostile_worker.lua");
    let mut vm = vm();
    vm.set_args_json(&serde_json::json!({"path":path.to_str().unwrap()}).to_string())
        .unwrap();
    vm.load(
        "worker-owner.lua",
        br#"
        PARENT_SECRET='not inherited'
        local t=require('thread');local s=t.sync
        assert(s.lock_create('owner_lock'));assert(s.queue_create('ready',{depth=1,item_size=32}))
        assert(t.start(args.path,{nested={value=42},flag=true},{name='hostile',timeout_ms=0}))
        assert(s.queue_recv('ready',1000)=='ready\0binary')
        local ok,err=s.unlock('owner_lock');assert(ok==false and err=='not_owner')
        local ok,err=s.lock_delete('owner_lock');assert(ok==nil and err=='busy')
        assert(t.stop('hostile',1000));assert(s.lock('owner_lock',0));assert(s.unlock('owner_lock'))
        assert(s.lock_delete('owner_lock'));assert(s.queue_delete('ready'))
        assert(s.queue_create('pending'));local msg,err=s.queue_recv('pending',100000)
        error('interrupted wait returned unexpectedly: '..tostring(err))
    "#,
    )
    .unwrap();
    for _ in 0..5000 {
        let step = vm.resume();
        assert!(matches!(step, Step::Slice), "{step:?}");
        advance_ms(1);
    }
    vm.interrupt();
    let out = drive(&mut vm, u32::MAX);
    assert_eq!(out.step, Step::Interrupted);
}
