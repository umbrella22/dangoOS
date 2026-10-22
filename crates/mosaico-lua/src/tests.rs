//! Host tests: the same C sources and glue as the firmware, built with the
//! host compiler.

use alloc::{boxed::Box, string::String, vec::Vec};

use crate::*;

#[derive(Default)]
struct Capture {
    lines: Vec<String>,
}

impl Host for Capture {
    fn print(&mut self, line: &[u8]) {
        self.lines.push(String::from_utf8_lossy(line).into_owned());
    }
}

fn vm_with(limits: Limits) -> Vm {
    let mut vm = Vm::new(Box::new(Capture::default()), limits, &GLOBAL_ALLOCATOR, 1).unwrap();
    vm.register("test", &[("add", add), ("sleep", sleep), ("info", info)])
        .unwrap();
    vm
}

fn fresh() -> Vm {
    vm_with(Limits::default())
}

fn add(call: &mut Call) -> Result<Ret, Error> {
    let a = call.number(1)?;
    let b = call.number(2)?;
    call.push_number(a + b);
    Ok(Ret::Values(1))
}

fn sleep(call: &mut Call) -> Result<Ret, Error> {
    let ms = call.integer(1)?;
    if ms < 0 {
        return Err(Error::msg("negative delay"));
    }
    Ok(Ret::Yield(Request::Sleep { ms: ms as u32 }))
}

fn info(call: &mut Call) -> Result<Ret, Error> {
    let lines = call.host::<Capture>().map(|c| c.lines.len()).unwrap_or(0);
    call.push_table(0, 2);
    call.set_str("name", "mosaico");
    call.set_integer("lines", lines as i64);
    Ok(Ret::Values(1))
}

/// Runs to completion, answering every request immediately.
fn run(vm: &mut Vm, source: &str) -> (Step, Vec<Request>) {
    vm.load("app.lua", source.as_bytes()).unwrap();
    let mut requests = Vec::new();
    for _ in 0..100_000 {
        match vm.resume() {
            Step::Slice => {}
            Step::Request(r) => requests.push(r),
            other => return (other, requests),
        }
    }
    panic!("script did not finish");
}

fn lines(vm: &mut Vm) -> Vec<String> {
    vm.host_mut::<Capture>().unwrap().lines.clone()
}

#[test]
fn prints_hello() {
    let mut vm = fresh();
    assert_eq!(run(&mut vm, "print('hi')").0, Step::Done);
    assert_eq!(lines(&mut vm), ["hi"]);
}

#[test]
fn print_formats_like_the_reference_interpreter() {
    let mut vm = fresh();
    let src = "print(1, 2.5, 1e100, 3 // 2, 7 / 2, true, nil, -0.0, math.maxinteger, 2^63)";
    assert_eq!(run(&mut vm, src).0, Step::Done);
    assert_eq!(
        lines(&mut vm),
        ["1\t2.5\t1e+100\t1\t3.5\ttrue\tnil\t-0.0\t9223372036854775807\t9.2233720368547758e+18"]
    );
}

#[test]
fn standard_libraries_present() {
    let mut vm = fresh();
    let src = r#"
        assert(_VERSION == "Lua 5.5")
        assert(string.format("%5.2f", math.pi) == " 3.14")
        assert(tonumber("0x10") == 16 and tonumber("1e3") == 1000.0)
        assert(utf8.char(0x72d0) == "\u{72d0}")
        assert(#string.pack("<i4d", 1, 2.5) == 12)
        local t = {5, 3, 9}; table.sort(t); assert(table.concat(t, ",") == "3,5,9")
        local co = coroutine.wrap(function(a) local b = coroutine.yield(a + 1); return b * 2 end)
        assert(co(1) == 2 and co(10) == 20)
        assert(type(debug.traceback()) == "string")
        assert(string.find("fox dango", "d(%a+)") == 5)
        assert(type(io.open) == "function" and dofile ~= nil and os.exit == nil and type(os.time()) == "number")
        assert(#package.searchers == 2)
        assert(debug.sethook == nil and debug.getinfo ~= nil)
        assert(require("test") == test)
        assert(not pcall(require, "nonexistent"))
        print("ok")
    "#;
    assert_eq!(run(&mut vm, src).0, Step::Done);
    assert_eq!(lines(&mut vm), ["ok"]);
}

#[test]
fn host_functions_and_requests() {
    let mut vm = fresh();
    let (step, requests) = run(
        &mut vm,
        "test.sleep(5); print(test.add(2, 3)); test.sleep(0); print(test.info().lines)",
    );
    assert_eq!(step, Step::Done);
    assert_eq!(
        requests,
        [Request::Sleep { ms: 5 }, Request::Sleep { ms: 0 }]
    );
    assert_eq!(lines(&mut vm), ["5.0", "1"]);
}

#[test]
fn argument_errors_name_the_function() {
    let mut vm = fresh();
    match run(&mut vm, "test.add('x', 1)").0 {
        Step::Error(m) => {
            assert!(
                m.starts_with("app.lua:1: bad argument #1 to 'add' (number expected, got string)"),
                "{m}"
            )
        }
        other => panic!("{other:?}"),
    }
    let mut vm = fresh();
    match run(&mut vm, "test.sleep(1.5)").0 {
        Step::Error(m) => assert!(m.contains("number has no integer representation"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn host_errors_carry_position_and_can_be_caught() {
    let mut vm = fresh();
    let src = "local ok, e = pcall(test.sleep, -1)\nprint(ok, e)\ntest.sleep(-1)";
    match run(&mut vm, src).0 {
        Step::Error(m) => {
            assert!(m.starts_with("app.lua:3: negative delay"), "{m}");
            assert!(m.contains("stack traceback:"), "{m}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(lines(&mut vm), ["false\tnegative delay"]);
}

#[test]
fn syntax_errors_fail_load() {
    let mut vm = fresh();
    let e = vm.load("bad.lua", b"x = = 1").unwrap_err();
    assert!(e.starts_with("bad.lua:1:"), "{e}");
    assert_eq!(vm.resume(), Step::Done);
}

#[test]
fn binary_chunks_are_refused() {
    let mut vm = fresh();
    let e = vm.load("bin.lua", b"\x1bLua\x55\x00").unwrap_err();
    assert!(e.contains("binary chunk"), "{e}");
}

#[test]
fn busy_loops_are_sliced() {
    let mut vm = vm_with(Limits {
        hook_interval: 100,
        slice_hooks: 2,
        ..Limits::default()
    });
    vm.load("loop.lua", b"local n = 0 while true do n = n + 1 end")
        .unwrap();
    for _ in 0..50 {
        assert_eq!(vm.resume(), Step::Slice);
    }
    vm.interrupt();
    assert_eq!(vm.resume(), Step::Interrupted);
    assert!(vm.finished());
}

#[test]
fn pcall_cannot_swallow_an_interrupt() {
    let mut vm = vm_with(Limits {
        hook_interval: 100,
        slice_hooks: 2,
        ..Limits::default()
    });
    vm.load(
        "stubborn.lua",
        b"while true do pcall(function() while true do end end) end",
    )
    .unwrap();
    assert_eq!(vm.resume(), Step::Slice);
    vm.interrupt();
    assert_eq!(vm.resume(), Step::Interrupted);
}

#[test]
fn unyieldable_loops_are_stopped() {
    let mut vm = vm_with(Limits {
        hook_interval: 100,
        slice_hooks: 2,
        max_blocked_hooks: 50,
        ..Limits::default()
    });
    let src = "table.sort({3, 2, 1}, function(a, b) while true do end end)";
    match run(&mut vm, src).0 {
        Step::Error(m) => assert!(m.contains("ran too long without yielding"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn sleeping_in_a_nested_coroutine_does_not_suspend_the_script() {
    let mut vm = fresh();
    let (step, requests) = run(
        &mut vm,
        "coroutine.wrap(function() test.sleep(1) end)() print('after')",
    );
    assert_eq!(step, Step::Done);
    assert!(requests.is_empty());
    assert_eq!(lines(&mut vm), ["after"]);
}

#[test]
fn memory_cap_is_enforced() {
    let cap = 256 * 1024;
    let mut vm = vm_with(Limits {
        memory_bytes: cap,
        ..Limits::default()
    });
    let src = "local t = {} for i = 1, 1e7 do t[i] = string.rep('x', 64) .. i end";
    match run(&mut vm, src).0 {
        Step::Error(m) => assert!(m.contains("not enough memory"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(vm.memory_peak() <= cap);
    assert!(vm.memory_used() > 0);
}

#[test]
fn script_coroutine_yield_is_a_slice() {
    let mut vm = fresh();
    vm.load("y.lua", b"coroutine.yield(1, 2) print('resumed')")
        .unwrap();
    assert_eq!(vm.resume(), Step::Slice);
    assert_eq!(vm.resume(), Step::Done);
    assert_eq!(lines(&mut vm), ["resumed"]);
}

#[test]
fn modules_can_extend_existing_tables() {
    let mut vm = fresh();
    vm.register("string", &[("add", add)]).unwrap();
    assert_eq!(
        run(&mut vm, "print(string.add(1, 2), string.upper('a'))").0,
        Step::Done
    );
    assert_eq!(lines(&mut vm), ["3.0\tA"]);
}

/// A clock that advances 10 us per reading, i.e. per hook call.
fn fake_clock() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static NOW: AtomicU64 = AtomicU64::new(0);
    NOW.fetch_add(10, Ordering::Relaxed)
}

#[test]
fn clocked_slices_follow_time_not_counts() {
    let mut vm = vm_with(Limits {
        hook_interval: 10,
        slice_hooks: 1, // ignored when a clock is set
        clock: Some(fake_clock),
        slice_us: 1_000,
        ..Limits::default()
    });
    // ~100 hook calls (1000 us of fake time) per slice: the loop needs many
    // hooks, so it must take several slices, not one per hook.
    vm.load("t.lua", b"for i = 1, 20000 do end").unwrap();
    let mut slices = 0;
    while vm.resume() == Step::Slice {
        slices += 1;
    }
    assert!(vm.finished());
    assert!((5..=300).contains(&slices), "{slices}");
}

#[test]
fn clocked_blocked_limit() {
    let mut vm = vm_with(Limits {
        hook_interval: 10,
        clock: Some(fake_clock),
        max_blocked_us: 5_000,
        ..Limits::default()
    });
    match run(
        &mut vm,
        "table.sort({3, 2, 1}, function(a, b) while true do end end)",
    )
    .0
    {
        Step::Error(m) => assert!(m.contains("ran too long without yielding"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn per_vm_root_interleaves_and_survives_finalizers() {
    use std::sync::{Arc, Mutex};
    struct Roots(Arc<Mutex<Vec<String>>>);
    impl Host for Roots {
        fn print(&mut self, text: &[u8]) {
            self.0
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(text).into_owned());
        }
    }
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut a = Vm::new(
        Box::new(Roots(captured.clone())),
        Limits::default(),
        &GLOBAL_ALLOCATOR,
        1,
    )
    .unwrap();
    let mut b = Vm::new(
        Box::new(Roots(captured.clone())),
        Limits::default(),
        &GLOBAL_ALLOCATOR,
        2,
    )
    .unwrap();
    a.set_storage_root("/data/appdata/a").unwrap();
    b.set_storage_root("/data/appdata/b").unwrap();
    assert!(a.set_storage_root("/bad\0root").is_err());
    assert!(a
        .set_storage_root(&alloc::format!("/{}", "x".repeat(128)))
        .is_err());
    let source = br#"
        local storage = require('storage')
        keeper = setmetatable({}, {__gc=function() print('gc:' .. storage.get_root_dir()) end})
        print(storage.get_root_dir())
        coroutine.yield()
        print(storage.get_root_dir())
    "#;
    a.load("a", source).unwrap();
    b.load("b", source).unwrap();
    a.resume();
    b.resume();
    a.resume();
    b.resume();
    drop(b);
    drop(a);
    let lines = captured.lock().unwrap();
    assert_eq!(
        &*lines,
        &[
            "/data/appdata/a",
            "/data/appdata/b",
            "/data/appdata/a",
            "/data/appdata/b",
            "gc:/data/appdata/b",
            "gc:/data/appdata/a"
        ]
    );
}

#[test]
fn compatibility_default_is_a_validated_owned_snapshot() {
    set_data_root("/data/default-before").unwrap();
    let mut before = fresh();
    set_data_root("/data/default-after").unwrap();
    let mut after = fresh();
    // Restore configuration immediately so concurrently constructed unrelated
    // VMs do not retain this test's default longer than necessary.
    set_data_root("/data").unwrap();
    assert!(set_data_root("relative").is_err());
    assert!(set_data_root("/bad\0root").is_err());
    assert!(set_data_root(&alloc::format!("/{}", "x".repeat(128))).is_err());
    before
        .load(
            "before",
            br#"assert(require('storage').get_root_dir() == '/data/default-before')"#,
        )
        .unwrap();
    after
        .load(
            "after",
            br#"assert(require('storage').get_root_dir() == '/data/default-after')"#,
        )
        .unwrap();
    assert_eq!(before.resume(), Step::Done);
    assert_eq!(after.resume(), Step::Done);
}
