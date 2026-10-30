#![allow(dead_code)]
use std::{cell::{Cell, RefCell}, collections::VecDeque, ffi::c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, AtomicU32, Ordering};
macro_rules! trace { ($($arg:tt)*) => {} }
macro_rules! info { ($($arg:tt)*) => { let _ = format_args!($($arg)*); } }
macro_rules! unwrap { ($value:expr, $message:literal) => { $value.expect($message) } }
const OSI_FUNCS_TIME_BLOCKING: u32 = u32::MAX;
static EVENTQ_DEPTH: AtomicU16 = AtomicU16::new(4);
thread_local! { static ISR: Cell<bool> = const { Cell::new(false) }; }
fn in_isr() -> bool { ISR.get() }
fn blob_ticks_to_micros(t: u32) -> u32 { t * 1_000 }

struct Queue(RefCell<VecDeque<*mut BtdmOsalPtr>>);
fn insert(q: *mut c_void, p: *const c_void, front: bool) -> i32 {
    let q = unsafe { &*q.cast::<Queue>() };
    let mut q = q.0.borrow_mut();
    if q.len() == 4 { return 0; }
    let p = unsafe { *p.cast::<*mut BtdmOsalPtr>() };
    if front { q.push_front(p) } else { q.push_back(p) }
    1
}
fn queue_send_to_back(q: *mut c_void, p: *const c_void, _: u32) -> i32 { insert(q, p, false) }
fn queue_send_to_front(q: *mut c_void, p: *const c_void, _: u32) -> i32 { insert(q, p, true) }
fn queue_try_send_to_back_from_isr(q: *mut c_void, p: *const c_void, _: *mut bool) -> i32 { insert(q, p, false) }
fn queue_try_send_to_front_from_isr(q: *mut c_void, p: *const c_void, _: *mut bool) -> i32 { insert(q, p, true) }
fn queue_receive(q: *mut c_void, p: *mut c_void, _: u32) -> i32 {
    let q = unsafe { &*q.cast::<Queue>() };
    if let Some(event) = q.0.borrow_mut().pop_front() {
        unsafe { p.cast::<*mut BtdmOsalPtr>().write(event) };
        1
    } else { 0 }
}
fn queue_try_receive_from_isr(q: *mut c_void, p: *mut c_void, _: *mut bool) -> i32 { queue_receive(q, p, 0) }
fn queue_remove(q: *mut c_void, p: *const c_void) {
    let q = unsafe { &*q.cast::<Queue>() };
    let event = unsafe { *p.cast::<*mut BtdmOsalPtr>() };
    q.0.borrow_mut().retain(|&p| p != event);
}
fn queue_messages_waiting(q: *mut c_void) -> u32 { unsafe { &*q.cast::<Queue>() }.0.borrow().len() as u32 }

#[test]
fn pending_event_is_coalesced_across_isr_task_front_and_back_posts() {
    let queue = Queue(RefCell::new(VecDeque::new()));
    let mut q = BtdmOsalPtr { ptr: (&queue as *const Queue).cast_mut().cast() };
    let mut event = Event { queued: AtomicBool::new(false), fn_ptr: None, arg: AtomicPtr::new(std::ptr::null_mut()) };
    let mut ev = BtdmOsalPtr { ptr: (&mut event as *mut Event).cast() };
    let mut event2 = Event { queued: AtomicBool::new(false), fn_ptr: None, arg: AtomicPtr::new(std::ptr::null_mut()) };
    let mut ev2 = BtdmOsalPtr { ptr: (&mut event2 as *mut Event).cast() };
    for i in 0..10_000 {
        ISR.set(i % 2 == 0);
        wr_btdm_osal_eventq_put(&mut q, &mut ev);
        wr_btdm_osal_eventq_put_to_front(&mut q, &mut ev);
    }
    assert!(wr_btdm_osal_event_is_queued(&mut ev));
    assert_eq!(queue_messages_waiting(q.ptr), 1);
    wr_btdm_osal_eventq_put_to_front(&mut q, &mut ev2);
    assert_eq!(wr_btdm_osal_eventq_get(&mut q, 0), &mut ev2 as *mut _);
    assert!(!wr_btdm_osal_event_is_queued(&mut ev2));
    assert_eq!(wr_btdm_osal_eventq_get(&mut q, 0), &mut ev as *mut _);
    assert!(!wr_btdm_osal_event_is_queued(&mut ev));
    assert!(wr_btdm_osal_eventq_get(&mut q, 0).is_null());
    ISR.set(false);
    wr_btdm_osal_eventq_put(&mut q, &mut ev);
    wr_btdm_osal_eventq_remove(&mut q, &mut ev);
    assert!(!wr_btdm_osal_event_is_queued(&mut ev));
    assert_eq!(queue_messages_waiting(q.ptr), 0);
    wr_btdm_osal_eventq_put(&mut q, &mut ev);
    assert_eq!(wr_btdm_osal_eventq_get(&mut q, 0), &mut ev as *mut _);
    wr_btdm_osal_event_set_arg(&mut ev, 42usize as *mut c_void);
    assert_eq!(wr_btdm_osal_event_get_arg(&mut ev), 42usize as *mut c_void);
}

#[test]
fn distinct_event_overflow_still_fails_and_releases_its_reservation() {
    ISR.set(true);
    let queue = Queue(RefCell::new(VecDeque::new()));
    let mut q = BtdmOsalPtr { ptr: (&queue as *const Queue).cast_mut().cast() };
    let mut events: Vec<_> = (0..5).map(|_| Box::new(Event {
        queued: AtomicBool::new(false), fn_ptr: None,
        arg: AtomicPtr::new(std::ptr::null_mut()),
    })).collect();
    let mut handles: Vec<_> = events.iter_mut().map(|e| BtdmOsalPtr { ptr: (&mut **e as *mut Event).cast() }).collect();
    for event in &mut handles[..4] { wr_btdm_osal_eventq_put(&mut q, event); }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wr_btdm_osal_eventq_put(&mut q, &mut handles[4]);
    }));
    assert!(result.is_err());
    assert!(!wr_btdm_osal_event_is_queued(&mut handles[4]));
    assert_eq!(queue_messages_waiting(q.ptr), 4);
}
