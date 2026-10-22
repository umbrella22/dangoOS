//! Host tests for the esp-claw compatible modules: the official display
//! contract, delay/system/os/json/arg_schema, and the Snake and 2048 apps
//! from esp-claw-skills-lab running unchanged.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
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
                }
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
    assert!(s
        .lines
        .iter()
        .any(|l| l.starts_with("[display_contract] buffers=2")));
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
    assert!(s
        .lines
        .iter()
        .any(|line| line.contains("hardware unavailable")));
    assert!(s.frame.contains(&0x07e0), "opaque green text was drawn");
    assert!(s.frame.contains(&0xffff), "opaque white title was drawn");
    assert!(!s.open, "example closes display");
}
