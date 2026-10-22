//! External-process worker lifecycle checks with a counted Lua/native allocator.
//! No firmware, serial port, or real-time sleeping is used.
use mosaico_lua::{Host, Limits, RawAllocator, Request, Step, Vm};
use std::{
    alloc::{Layout, alloc, dealloc},
    env, fs,
    path::Path,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

static TIME: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
fn clock() -> u64 {
    TIME.load(Ordering::Relaxed)
}
unsafe fn allocate(layout: Layout) -> *mut u8 {
    let ptr = unsafe { alloc(layout) };
    if !ptr.is_null() {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
    }
    ptr
}
unsafe fn deallocate(ptr: *mut u8, layout: Layout) {
    LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    unsafe { dealloc(ptr, layout) };
}
static ALLOCATOR: RawAllocator = RawAllocator {
    alloc: allocate,
    dealloc: deallocate,
};
struct Output;
impl Host for Output {
    fn print(&mut self, bytes: &[u8]) {
        println!("{}", String::from_utf8_lossy(bytes));
    }
}
fn vm(root: &Path, source: &str, token: &str) -> Vm {
    let mut limits = Limits::default();
    limits.clock = Some(clock);
    limits.memory_bytes = 256 * 1024;
    let mut vm = Vm::new(Box::new(Output), limits, &ALLOCATOR, 0).unwrap();
    vm.set_storage_root(root.to_str().unwrap()).unwrap();
    vm.set_args_json(&serde_json::json!({"root":root.to_str().unwrap(),"token":token}).to_string())
        .unwrap();
    vm.load("independent-worker-owner", source.as_bytes())
        .unwrap();
    vm
}
fn drive(vm: &mut Vm) {
    for _ in 0..100_000 {
        TIME.fetch_add(1000, Ordering::Relaxed);
        match vm.resume() {
            Step::Done => return,
            Step::Slice => {}
            Step::Request(Request::Sleep { ms }) => {
                for _ in 0..ms {
                    TIME.fetch_add(1000, Ordering::Relaxed);
                    vm.poll_workers();
                }
            }
            other => panic!("worker owner failed: {other:?}"),
        }
    }
    panic!("worker owner did not finish");
}
fn main() {
    let args: Vec<_> = env::args().collect();
    assert_eq!(args.len(), 2, "n5_worker_probe <new-output-dir>");
    fs::create_dir(&args[1]).unwrap();
    let root = fs::canonicalize(&args[1]).unwrap();
    for (name, source) in [
        (
            "respond",
            "assert(SECRET==nil);assert(io==nil and package==nil);require('delay').delay_ms(5);assert(require('thread').sync.queue_send('same',args.token..'\\0done',100));print('reply '..args.token)",
        ),
        (
            "oom",
            "local keep={};while true do keep[#keep+1]=string.rep('x',4096) end",
        ),
        (
            "loop",
            "local guard <close> = setmetatable({},{__close=function()while true do end end});pcall(function()while true do end end)",
        ),
        (
            "locked",
            "assert(require('thread').sync.lock('held',0));assert(require('thread').sync.queue_send('ready','locked',0));require('delay').delay_ms(10000)",
        ),
        ("done", "print('done')"),
    ] {
        fs::write(root.join(format!("{name}.lua")), source).unwrap();
    }
    let source = r#"
        SECRET='parent only'
        local t=require('thread');local s=t.sync
        local function file(name)return args.root..'/'..name..'.lua'end
        assert(s.queue_create('same'))
        assert(t.start(file('respond'),{token=args.token},{name='answer'}))
        require('delay').delay_ms(100)
        assert(s.queue_recv('same',0)==args.token..'\0done')
        local ok,status=t.get('answer');assert(ok and status:find('status=done',1,true))
        ok,status=t.run(file('oom'),{},{name='oom'});assert(ok==nil and status:find('status=failed',1,true))
        assert(s.queue_create('ready'));assert(s.lock_create('held'))
        assert(t.start(file('locked'),{},{name='held',timeout_ms=100}))
        assert(s.queue_recv('ready',100)=='locked')
        local locked,why=s.lock('held',1);assert(locked==false and why=='timeout')
        assert(s.lock('held',1000));assert(s.unlock('held'))
        ok,status=t.get('held');assert(ok and status:find('status=timeout',1,true))
        ok,status=t.run(file('loop'),{},{name='caught-loop',timeout_ms=50})
        assert(ok==nil and (status:find('status=timeout',1,true) or status:find('status=failed',1,true)))
        for i=1,40 do assert(t.run(file('done'),{},{name='history_'..i})) end
        ok,status=t.list();assert(ok)
        local _,count=status:gsub('id=','');assert(count<=32,count)
        print('WORKER_LIFECYCLE_COMPLETE '..args.token)
    "#;
    let mut a = vm(&root, source, "A");
    drive(&mut a);
    drop(a);
    assert_eq!(
        LIVE.load(Ordering::Relaxed),
        0,
        "worker group leaked after completion"
    );

    let isolated = r#"
        local t=require('thread');assert(t.sync.queue_create('same'))
        assert(t.start(args.root..'/respond.lua',{token=args.token},{name='shared-name'}))
        assert(t.sync.queue_recv('same',1000)==args.token..'\0done')
        print('GROUP_ISOLATION_COMPLETE '..args.token)
    "#;
    let mut a = vm(&root, isolated, "left");
    let mut b = vm(&root, isolated, "right");
    let mut done = [false; 2];
    for _ in 0..10_000 {
        TIME.fetch_add(1000, Ordering::Relaxed);
        for (i, v) in [&mut a, &mut b].into_iter().enumerate() {
            if done[i] {
                continue;
            }
            match v.resume() {
                Step::Done => done[i] = true,
                Step::Slice => {}
                other => panic!("isolation: {other:?}"),
            }
        }
        if done.iter().all(|v| *v) {
            break;
        }
    }
    assert_eq!(done, [true, true]);
    drop(a);
    drop(b);
    assert_eq!(LIVE.load(Ordering::Relaxed), 0, "independent groups leaked");

    let mut suspended = vm(
        &root,
        r#"
        local t=require('thread');assert(t.sync.queue_create('blocked'))
        assert(t.start(args.root..'/loop.lua',{},{name='endless',timeout_ms=0}))
        t.sync.queue_recv('blocked',100000)
        error('owner should be destroyed while blocked')
    "#,
        "drop",
    );
    for _ in 0..30 {
        TIME.fetch_add(1000, Ordering::Relaxed);
        assert_eq!(suspended.resume(), Step::Slice);
    }
    drop(suspended);
    assert_eq!(
        LIVE.load(Ordering::Relaxed),
        0,
        "drop left a child VM allocated"
    );
    println!("WORKER_GROUP_DISCARD_COMPLETE live_bytes=0");
}
