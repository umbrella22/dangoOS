#![cfg(feature = "ts-engine")]
//! Paired VM churn on the device TLSF implementation; no device evidence implied.
#[path = "../../../vendor/esp-alloc/src/heap/tlsf.rs"]
mod tlsf;

use core::{alloc::Layout, ptr::NonNull};
use std::{cell::RefCell, rc::Rc};
use tlsf::TlsfHeap;

struct Pool {
    _bytes: Box<[u64]>,
    heap: TlsfHeap,
    allocations: usize,
    failures: usize,
    peak: usize,
}
impl Pool {
    fn new() -> Self {
        let mut bytes = vec![0u64; 2 * 1024 * 1024].into_boxed_slice();
        let heap = unsafe { TlsfHeap::new(bytes.as_mut_ptr().cast(), bytes.len() * 8) };
        Self {
            _bytes: bytes,
            heap,
            allocations: 0,
            failures: 0,
            peak: 0,
        }
    }
    fn sample(&self) -> serde_json::Value {
        let s = self.heap.block_stats();
        assert_eq!(s.free, self.heap.free());
        serde_json::json!({"used":s.used,"free":s.free,"largest":s.largest,
            "allocated_blocks":s.occupied_blocks,"free_blocks":s.free_blocks,
            "allocations":self.allocations,"failures":self.failures,"peak":self.peak})
    }
}
thread_local! {
    static POOL: RefCell<Option<Pool>> = const { RefCell::new(None) };
}
unsafe fn allocate(layout: Layout) -> *mut u8 {
    POOL.with_borrow_mut(|pool| {
        let pool = pool.as_mut().unwrap();
        let p = pool.heap.allocate(layout);
        if p.is_some() {
            pool.allocations += 1;
        } else {
            pool.failures += 1;
        }
        pool.peak = pool.peak.max(pool.heap.used());
        p.map_or(core::ptr::null_mut(), NonNull::as_ptr)
    })
}

#[test]
fn extended_resident_churn_retains_large_allocation_capacity() {
    let lua = std::str::from_utf8(LUA)
        .unwrap()
        .replace("n=0,63 do", "n=0,4095 do")
        .replace("print(n)", "if n%64==63 then print(n) end")
        .replace("print(64)", "print(4096)");
    let package = include_bytes!("../../../artifacts/embedts/fixtures/memory-resident.etb");
    let mut reports = Vec::new();
    for (engine, backing) in [
        ("lua", &BACKING),
        ("ts-copy", &COPY_BACKING),
        ("ts", &BACKING),
    ] {
        POOL.with_borrow_mut(|pool| *pool = Some(Pool::new()));
        let before = sample();
        let observations = Rc::new(RefCell::new(Vec::new()));
        if engine == "lua" {
            let mut vm = lua_engine::Vm::new(
                Box::new(Capture(observations.clone())),
                lua_engine::Limits {
                    memory_bytes: 4 * 1024 * 1024,
                    ..Default::default()
                },
                backing,
                1,
            )
            .unwrap();
            vm.load("resident-churn", lua.as_bytes()).unwrap();
            loop {
                match vm.resume() {
                    lua_engine::Step::Slice => {}
                    lua_engine::Step::Done => break,
                    other => panic!("{other:?}"),
                }
            }
            drop(vm);
        } else {
            let mut vm = ts_engine::Vm::new(
                Box::new(Capture(observations.clone())),
                ts_engine::Limits {
                    memory_bytes: 4 * 1024 * 1024,
                    interrupt_polls: 100_000,
                    ..Default::default()
                },
                &os::apps::ts_host::allocator(backing),
            )
            .unwrap();
            unsafe { vm.load(ts_engine::Package::parse(package).unwrap()) }.unwrap();
            assert_eq!(vm.call("main", None).unwrap(), Some(42));
            assert_eq!(vm.close().used, 0);
        }
        assert_eq!(observations.borrow().len(), 66);
        for observation in observations.borrow().iter() {
            // A multi-MiB contiguous reserve must survive the entire allocation
            // history, including strings retained across churn rounds.
            assert!(observation["heap"]["largest"].as_u64().unwrap() > 8 * 1024 * 1024);
            assert_eq!(observation["heap"]["failures"], 0);
        }
        let after = sample();
        assert_eq!(after["used"], 0);
        assert_eq!(after["free"], before["free"]);
        assert_eq!(after["largest"], before["largest"]);
        assert_eq!(after["free_blocks"], before["free_blocks"]);
        reports.push(serde_json::json!({"engine":engine,"rounds":4096,
            "after_close":after,"during":*observations.borrow()}));
        POOL.with_borrow_mut(|pool| *pool = None);
    }
    println!(
        "SCRIPT_MEMORY_RESIDENT {}",
        serde_json::json!({"scope":"host TLSF; extended allocation history; no physical duration claim", "runs":reports})
    );
}
unsafe fn release(p: *mut u8, layout: Layout) {
    POOL.with_borrow_mut(|pool| {
        assert!(unsafe {
            pool.as_mut()
                .unwrap()
                .heap
                .try_deallocate(NonNull::new(p).unwrap(), layout)
        });
    });
}
unsafe fn resize(p: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
    POOL.with_borrow_mut(|pool| {
        let pool = pool.as_mut().unwrap();
        assert!(pool.heap.contains(NonNull::new(p).unwrap()));
        let result = unsafe {
            pool.heap.reallocate(
                NonNull::new(p).unwrap(),
                Layout::from_size_align(new_size, old.align()).unwrap(),
            )
        };
        if let Some(new) = result {
            if new.as_ptr() != p {
                pool.allocations += 1;
            }
        } else {
            pool.failures += 1;
        }
        pool.peak = pool.peak.max(pool.heap.used());
        result.map_or(core::ptr::null_mut(), NonNull::as_ptr)
    })
}
static BACKING: app_host::RawAllocator = app_host::RawAllocator {
    alloc: allocate,
    dealloc: release,
    realloc: Some(resize),
};
static COPY_BACKING: app_host::RawAllocator = app_host::RawAllocator {
    alloc: allocate,
    dealloc: release,
    realloc: None,
};
fn sample() -> serde_json::Value {
    POOL.with_borrow(|pool| pool.as_ref().unwrap().sample())
}
type Samples = Rc<RefCell<Vec<serde_json::Value>>>;
struct Capture(Samples);
impl ts_engine::Host for Capture {
    fn emit(&mut self, value: i32) -> Result<(), ts_engine::Error> {
        self.0
            .borrow_mut()
            .push(serde_json::json!({"phase":value,"heap":sample()}));
        Ok(())
    }
}
impl lua_engine::Host for Capture {
    fn print(&mut self, line: &[u8]) {
        let phase: i32 = std::str::from_utf8(line).unwrap().parse().unwrap();
        self.0
            .borrow_mut()
            .push(serde_json::json!({"phase":phase,"heap":sample()}));
    }
}
const LUA: &[u8] = br#"
local live={}
for i=0,31 do live[i+1]=string.rep('x',1024+i*31) end
print(-1)
for n=0,63 do
    local temporary={}
    for i=0,63 do temporary[i+1]=string.rep('x',32+(i*193+n*31)%4096) end
    local objects={}
    for i=0,127 do objects[i+1]={x=i,y=n,text=temporary[i%64+1]} end
    live[n%32+1]=string.rep('x',128+(n*97)%8192)
    assert(objects[128].x==127 and #temporary==64)
    print(n)
end
print(64)
"#;

#[test]
fn paired_churn_and_repeated_vm_lifetimes_restore_the_tlsf_pool() {
    let package = include_bytes!("../../../artifacts/embedts/fixtures/memory-churn.etb");
    let mut reports = Vec::new();
    for (engine, backing) in [
        ("lua", &BACKING),
        ("ts-copy", &COPY_BACKING),
        ("ts", &BACKING),
    ] {
        POOL.with_borrow_mut(|pool| *pool = Some(Pool::new()));
        let before = sample();
        let observations = Rc::new(RefCell::new(Vec::new()));
        for cycle in 0..8 {
            let start = observations.borrow().len();
            let vm_stats = if engine != "lua" {
                let mut vm = ts_engine::Vm::new(
                    Box::new(Capture(observations.clone())),
                    ts_engine::Limits {
                        memory_bytes: 4 * 1024 * 1024,
                        interrupt_polls: 100_000,
                        ..Default::default()
                    },
                    &os::apps::ts_host::allocator(backing),
                )
                .unwrap();
                unsafe { vm.load(ts_engine::Package::parse(package).unwrap()) }.unwrap();
                assert_eq!(vm.call("main", None).unwrap(), Some(42));
                let stats = vm.close();
                assert_eq!(stats.used, 0);
                serde_json::json!({"used":stats.used,"peak":stats.peak,
                    "allocations":stats.allocations,"reallocations":stats.reallocations,
                    "moved":stats.realloc_moved,"copied_bytes":stats.realloc_copied_bytes})
            } else {
                let mut vm = lua_engine::Vm::new(
                    Box::new(Capture(observations.clone())),
                    lua_engine::Limits {
                        memory_bytes: 4 * 1024 * 1024,
                        ..Default::default()
                    },
                    &BACKING,
                    1,
                )
                .unwrap();
                vm.load("memory-churn", LUA).unwrap();
                loop {
                    match vm.resume() {
                        lua_engine::Step::Slice => {}
                        lua_engine::Step::Done => break,
                        other => panic!("{other:?}"),
                    }
                }
                drop(vm);
                serde_json::Value::Null
            };
            assert_eq!(observations.borrow().len() - start, 66);
            let after = sample();
            assert_eq!(after["used"], 0);
            assert_eq!(after["free"], before["free"]);
            assert_eq!(after["largest"], before["largest"]);
            assert_eq!(after["free_blocks"], before["free_blocks"]);
            reports.push(
                serde_json::json!({"engine":engine,"cycle":cycle,"after_close":after,
                "vm":vm_stats,"during":&observations.borrow()[start..]}),
            );
        }
        POOL.with_borrow_mut(|pool| *pool = None);
    }
    println!(
        "SCRIPT_MEMORY {}",
        serde_json::json!({"scope":"host TLSF; native pointer width; no physical result",
        "heap_peak":"sampled after backing calls; excludes overlap inside native realloc",
        "vm_peak":"requested backing bytes, conservatively including old and new when moved", "runs":reports})
    );
}
