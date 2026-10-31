//! Memory bursts, allocation failure and recovery through the actual Lua VM.
use alloc::{boxed::Box, string::String, vec::Vec};
use core::{alloc::Layout, cell::RefCell, ptr};
use std::collections::BTreeMap;

use crate::{Call, Error, Host, Limits, RawAllocator, Ret, Step, Vm};

struct Allocations {
    live: BTreeMap<usize, Layout>,
    used: usize,
    peak: usize,
    capacity: usize,
    rejected: usize,
    layout_mismatches: usize,
}

std::thread_local! {
    static ALLOCATIONS: RefCell<Allocations> = RefCell::new(Allocations {
        live: BTreeMap::new(), used: 0, peak: 0, capacity: usize::MAX,
        rejected: 0, layout_mismatches: 0,
    });
}

unsafe fn allocate(layout: Layout) -> *mut u8 {
    ALLOCATIONS.with(|state| {
        let mut state = state.borrow_mut();
        if layout.size() > state.capacity.saturating_sub(state.used) {
            state.rejected += 1;
            return ptr::null_mut();
        }
        let pointer = alloc::alloc::alloc(layout);
        if !pointer.is_null() {
            state.live.insert(pointer as usize, layout);
            state.used += layout.size();
            state.peak = state.peak.max(state.used);
        }
        pointer
    })
}

unsafe fn deallocate(pointer: *mut u8, supplied: Layout) {
    ALLOCATIONS.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(actual) = state.live.remove(&(pointer as usize)) {
            state.layout_mismatches += usize::from(actual != supplied);
            state.used -= actual.size();
            alloc::alloc::dealloc(pointer, actual);
        } else {
            // Other host tests can use the process-wide C-module allocator.
            alloc::alloc::dealloc(pointer, supplied);
        }
    });
}

static TRACKED: RawAllocator = RawAllocator {
    alloc: allocate,
    dealloc: deallocate,
};
const APP_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct Output(Vec<String>);
impl Host for Output {
    fn print(&mut self, line: &[u8]) {
        self.0.push(String::from_utf8_lossy(line).into_owned());
    }
}

fn reset(capacity: usize) {
    ALLOCATIONS.with(|state| {
        let mut state = state.borrow_mut();
        assert_eq!(state.used, 0, "previous VM allocations must be released");
        state.peak = 0;
        state.capacity = capacity;
        state.rejected = 0;
        state.layout_mismatches = 0;
    });
}

fn make_vm(limit: usize) -> Vm {
    Vm::new(
        Box::<Output>::default(),
        Limits {
            memory_bytes: limit,
            ..Limits::default()
        },
        &TRACKED,
        1,
    )
    .unwrap()
}

fn drive(vm: &mut Vm, source: &str) -> Step {
    vm.load("memory-stress.lua", source.as_bytes()).unwrap();
    for _ in 0..100_000 {
        match vm.resume() {
            Step::Slice | Step::Request(_) => (),
            outcome => return outcome,
        }
    }
    panic!("memory fixture did not complete");
}

fn released() {
    ALLOCATIONS.with(|state| {
        let state = state.borrow();
        assert_eq!(state.used, 0);
        assert!(state.live.is_empty());
        assert_eq!(state.layout_mismatches, 0);
    });
}

#[test]
fn production_limit_rejects_a_single_burst_and_a_new_vm_still_runs() {
    reset(usize::MAX);
    let mut first = make_vm(APP_LIMIT);
    let result = drive(
        &mut first,
        "local data = string.rep('x', 8 * 1024 * 1024) print(#data)",
    );
    assert!(
        matches!(result, Step::Error(ref e) if e.contains("not enough memory")),
        "{result:?}"
    );
    assert!(first.memory_peak() <= APP_LIMIT);
    drop(first);
    released();
    let mut next = make_vm(APP_LIMIT);
    assert_eq!(drive(&mut next, "print('restart-ok')"), Step::Done);
    assert_eq!(next.host_mut::<Output>().unwrap().0, ["restart-ok"]);
    drop(next);
    released();
}

#[test]
fn caught_memory_exhaustion_releases_data_and_recovers_in_the_same_vm() {
    reset(usize::MAX);
    let mut vm = make_vm(APP_LIMIT);
    let result = drive(
        &mut vm,
        r#"
        for round = 1, 8 do
            local held = {}
            local ok, message = pcall(function()
                for i = 1, 1024 do held[i] = string.rep(string.char(i % 255), 32768) end
            end)
            assert(not ok and tostring(message):find('not enough memory', 1, true))
            held = nil
            collectgarbage('collect')
            assert(collectgarbage('count') < 256)
        end
        local data = string.rep('r', 512 * 1024)
        assert(#data == 512 * 1024)
        print('recovered-8-rounds')
    "#,
    );
    assert_eq!(result, Step::Done);
    assert!(vm.memory_peak() <= APP_LIMIT);
    assert_eq!(vm.host_mut::<Output>().unwrap().0, ["recovered-8-rounds"]);
    drop(vm);
    released();
}

#[test]
fn backing_allocator_exhaustion_below_the_vm_limit_is_a_lua_error() {
    reset(512 * 1024);
    let mut vm = make_vm(APP_LIMIT);
    let result = drive(
        &mut vm,
        "local t = {} for i = 1, 1024 do t[i] = string.rep('z', 32768) end",
    );
    assert!(
        matches!(result, Step::Error(ref e) if e.contains("not enough memory")),
        "{result:?}"
    );
    ALLOCATIONS.with(|state| {
        let state = state.borrow();
        assert!(state.rejected > 0);
        assert!(state.peak <= 512 * 1024);
    });
    drop(vm);
    released();
    reset(usize::MAX);
    let mut next = make_vm(APP_LIMIT);
    assert_eq!(drive(&mut next, "print('allocator-recovered')"), Step::Done);
    drop(next);
    released();
}

fn host_burst(call: &mut Call) -> Result<Ret, Error> {
    call.push_bytes(&alloc::vec![b'x'; 384 * 1024]);
    Ok(Ret::Values(1))
}

#[test]
fn rust_host_results_can_exceed_the_soft_vm_limit() {
    reset(usize::MAX);
    let cap = 256 * 1024;
    let mut vm = make_vm(cap);
    vm.register("probe", &[("burst", host_burst)]).unwrap();
    let result = drive(
        &mut vm,
        r#"
        local data = probe.burst()
        assert(#data == 384 * 1024)
        data = nil
        collectgarbage('collect')
        print('host-result-released')
    "#,
    );
    assert_eq!(result, Step::Done);
    assert!(
        vm.memory_peak() > cap,
        "host-function allocations currently bypass the cap"
    );
    drop(vm);
    released();
}
