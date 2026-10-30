//! esp_timer adapter with explicit callback quiescence. Timer handles and the
//! polling worker share Arc ownership; stop/delete wait outside all locks.
use alloc::{sync::Arc, vec::Vec};
use core::{
    cell::RefCell,
    ffi::{c_char, c_void},
    sync::atomic::{AtomicBool, Ordering},
};
use critical_section::Mutex;
use esp_radio_rtos_driver as rtos;
#[repr(C)]
pub struct Args {
    callback: unsafe extern "C" fn(*mut c_void),
    arg: *mut c_void,
    dispatch: i32,
    name: *const c_char,
    skip: bool,
}
struct Timing {
    deadline: u64,
    period: u64,
    active: bool,
    deleted: bool,
}
struct Timer {
    callback: unsafe extern "C" fn(*mut c_void),
    arg: usize,
    state: RefCell<Timing>,
    running: AtomicBool,
}
// State is accessed only inside a critical section; callback data belongs to C
// and is released only after delete() has joined its last invocation.
unsafe impl Send for Timer {}
unsafe impl Sync for Timer {}
static TIMERS: Mutex<RefCell<Vec<Arc<Timer>>>> = Mutex::new(RefCell::new(Vec::new()));
static STARTED: AtomicBool = AtomicBool::new(false);
pub fn init() {
    if !STARTED.swap(true, Ordering::AcqRel) {
        unsafe {
            rtos::task_create("bt_timers", worker, core::ptr::null_mut(), 2, Some(0), 4096);
        }
    }
}
extern "C" fn worker(_: *mut c_void) {
    loop {
        let due = critical_section::with(|cs| {
            let mut due = Vec::new();
            let now = rtos::now();
            for t in TIMERS.borrow_ref(cs).iter() {
                let mut s = t.state.borrow_mut();
                if s.active && !s.deleted && now >= s.deadline {
                    if s.period == 0 {
                        s.active = false;
                    } else {
                        s.deadline = now.saturating_add(s.period);
                    }
                    t.running.store(true, Ordering::Release);
                    due.push(t.clone());
                }
            }
            due
        });
        for timer in due {
            let call = critical_section::with(|_| !timer.state.borrow().deleted);
            if call {
                unsafe {
                    (timer.callback)(timer.arg as *mut c_void);
                }
            }
            timer.running.store(false, Ordering::Release);
        }
        rtos::usleep(1000);
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_timer_create(args: *const Args, out: *mut *const Timer) -> i32 {
    if args.is_null() || out.is_null() {
        return 0x102;
    }
    let args = &*args;
    let timer = Arc::new(Timer {
        callback: args.callback,
        arg: args.arg as usize,
        state: RefCell::new(Timing {
            deadline: 0,
            period: 0,
            active: false,
            deleted: false,
        }),
        running: AtomicBool::new(false),
    });
    critical_section::with(|cs| TIMERS.borrow_ref_mut(cs).push(timer.clone()));
    out.write(Arc::into_raw(timer));
    0
}
unsafe fn arm(p: *const Timer, us: u64, periodic: bool) -> i32 {
    if p.is_null() || us == 0 {
        return 0x102;
    }
    critical_section::with(|_| {
        let mut s = (*p).state.borrow_mut();
        if s.deleted || s.active {
            return 0x103;
        }
        s.deadline = rtos::now().saturating_add(us);
        s.period = if periodic { us } else { 0 };
        s.active = true;
        0
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_timer_once(p: *const Timer, us: u64) -> i32 {
    arm(p, us, false)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_timer_periodic(p: *const Timer, us: u64) -> i32 {
    arm(p, us, true)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_timer_stop(p: *const Timer) -> i32 {
    if p.is_null() {
        return 0x102;
    }
    let active = critical_section::with(|_| {
        let mut s = (*p).state.borrow_mut();
        let active = s.active;
        s.active = false;
        active
    });
    while (*p).running.load(Ordering::Acquire) {
        rtos::usleep(1000);
    }
    if active {
        0
    } else {
        0x103
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_timer_delete(p: *const Timer) -> i32 {
    if p.is_null() {
        return 0x102;
    }
    critical_section::with(|cs| {
        let mut s = (*p).state.borrow_mut();
        s.deleted = true;
        s.active = false;
        TIMERS.borrow_ref_mut(cs).retain(|t| Arc::as_ptr(t) != p);
    });
    while (*p).running.load(Ordering::Acquire) {
        rtos::usleep(1000);
    }
    drop(Arc::from_raw(p));
    0
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_timer_active(p: *const Timer) -> bool {
    !p.is_null() && critical_section::with(|_| (*p).state.borrow().active)
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_time_us() -> i64 {
    rtos::now() as i64
}
