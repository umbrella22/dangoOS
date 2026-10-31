//! Native scheduler adapters. Every potentially waiting operation polls without
//! holding a critical section, then yields to the existing esp-rtos scheduler.
use alloc::{boxed::Box, ffi::CString, vec::Vec};
use core::{
    cell::RefCell,
    ffi::{c_char, c_void, CStr},
    ptr::NonNull,
};
use critical_section::Mutex;
use esp_radio_rtos_driver as rtos;
use rtos::{
    queue::QueueHandle,
    semaphore::{SemaphoreHandle, SemaphoreKind},
};
unsafe extern "Rust" {
    fn esp_rtos_set_task_priority(task: NonNull<()>, priority: u32);
}
static NAMES: Mutex<RefCell<Vec<(usize, CString)>>> = Mutex::new(RefCell::new(Vec::new()));
fn retry(ms: u32, mut f: impl FnMut() -> bool) -> i32 {
    let deadline = rtos::now().saturating_add(u64::from(ms) * 1000);
    loop {
        if f() {
            return 1;
        }
        if ms != u32::MAX && rtos::now() >= deadline {
            return 0;
        }
        rtos::usleep(1000);
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_task_create(
    f: extern "C" fn(*mut c_void),
    name: *const c_char,
    stack: u32,
    arg: *mut c_void,
    prio: u32,
    out: *mut *mut c_void,
    _core: i32,
) -> i32 {
    unsafe {
        let name = CStr::from_ptr(name).to_string_lossy();
        let task = rtos::task_create(
            &name,
            f,
            arg,
            prio.min(rtos::max_task_priority()),
            Some(0),
            stack as usize,
        );

        let owned = CString::new(name.as_bytes()).unwrap();
        critical_section::with(|cs| {
            NAMES
                .borrow_ref_mut(cs)
                .push((task.as_ptr() as usize, owned))
        });
        if !out.is_null() {
            out.write(task.as_ptr().cast());
        }
        1
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_task_delete(task: *mut c_void) {
    unsafe {
        let task = if task.is_null() {
            rtos::current_task()
        } else {
            NonNull::new(task.cast()).unwrap()
        };
        critical_section::with(|cs| {
            NAMES
                .borrow_ref_mut(cs)
                .retain(|(id, _)| *id != task.as_ptr() as usize)
        });
        rtos::schedule_task_deletion(Some(task));
    }
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_task_delay(ms: u32) {
    rtos::usleep(ms.saturating_mul(1000));
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_task_priority(task: *mut c_void, p: u32) {
    unsafe {
        esp_rtos_set_task_priority(
            NonNull::new(task.cast()).unwrap_or_else(rtos::current_task),
            p.min(rtos::max_task_priority()),
        );
    }
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_task_current() -> *mut c_void {
    rtos::current_task().as_ptr().cast()
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_ticks() -> u32 {
    (rtos::now() / 1000) as u32
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_task_name(task: *mut c_void) -> *const c_char {
    critical_section::with(|cs| {
        NAMES
            .borrow_ref(cs)
            .iter()
            .find(|(id, _)| *id == task as usize)
            .map(|(_, s)| s.as_ptr())
            .unwrap_or(c"bluedroid".as_ptr())
    })
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_queue_create(cap: u32, size: u32) -> *mut c_void {
    Box::into_raw(Box::new(NativeQueue {
        handle: QueueHandle::new(cap as usize, size as usize),
        capacity: cap,
    }))
    .cast()
}
struct NativeQueue {
    handle: QueueHandle,
    capacity: u32,
}
unsafe fn queue<'a>(p: &'a NonNull<()>) -> &'a NativeQueue {
    unsafe { &*p.as_ptr().cast::<NativeQueue>() }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_queue_send(q: *mut c_void, p: *const u8, ms: u32) -> i32 {
    unsafe {
        let q = NonNull::new(q.cast()).unwrap();
        retry(ms, || queue(&q).handle.send_to_back(p, Some(0)))
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_queue_receive(q: *mut c_void, p: *mut u8, ms: u32) -> i32 {
    unsafe {
        let q = NonNull::new(q.cast()).unwrap();
        retry(ms, || queue(&q).handle.receive(p, Some(0)))
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_queue_count(q: *mut c_void) -> u32 {
    unsafe {
        queue(&NonNull::new(q.cast()).unwrap())
            .handle
            .messages_waiting() as u32
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_queue_spaces(q: *mut c_void) -> u32 {
    unsafe {
        {
            let ptr = NonNull::new(q.cast()).unwrap();
            let q = queue(&ptr);
            q.capacity - q.handle.messages_waiting() as u32
        }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_queue_delete(q: *mut c_void) {
    unsafe {
        if let Some(p) = NonNull::new(q.cast::<()>()) {
            drop(Box::from_raw(p.as_ptr().cast::<NativeQueue>()));
        }
    }
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_sem_create(max: u32, initial: u32, kind: u32) -> *mut c_void {
    SemaphoreHandle::new(match kind {
        1 => SemaphoreKind::Mutex,
        2 => SemaphoreKind::RecursiveMutex,
        _ => SemaphoreKind::Counting { max, initial },
    })
    .leak()
    .as_ptr()
    .cast()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_sem_take(p: *mut c_void, ms: u32) -> i32 {
    unsafe {
        let p = NonNull::new(p.cast()).unwrap();
        let sem = SemaphoreHandle::ref_from_ptr(&p);
        retry(ms, || sem.try_take())
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_sem_give(p: *mut c_void) -> i32 {
    unsafe { SemaphoreHandle::ref_from_ptr(&NonNull::new(p.cast()).unwrap()).give() as i32 }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_sem_delete(p: *mut c_void) {
    unsafe {
        if let Some(p) = NonNull::new(p.cast()) {
            drop(SemaphoreHandle::from_ptr(p));
        }
    }
}
unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn malloc_internal(size: usize) -> *mut c_void;
    fn calloc(n: u32, size: usize) -> *mut c_void;
    fn calloc_internal(n: u32, size: usize) -> *mut c_void;
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_heap_malloc(size: usize, caps: u32) -> *mut c_void {
    unsafe {
        if caps & (1 << 11) != 0 {
            malloc_internal(size)
        } else {
            malloc(size)
        }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_heap_calloc(n: usize, size: usize, caps: u32) -> *mut c_void {
    unsafe {
        if caps & (1 << 11) != 0 {
            calloc_internal(n as u32, size)
        } else {
            calloc(n as u32, size)
        }
    }
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_heap_free_size(caps: u32) -> usize {
    if caps & (1 << 11) != 0 {
        esp_alloc::HEAP.free_caps(esp_alloc::MemoryCapability::Internal.into())
    } else {
        esp_alloc::HEAP.free()
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_log_bytes(level: i32, tag: *const c_char, text: *const c_char) {
    unsafe {
        let tag = CStr::from_ptr(tag).to_string_lossy();
        let text = CStr::from_ptr(text).to_string_lossy();
        if level == 1 {
            log::error!("bt/{tag}: {text}");
        } else {
            log::warn!("bt/{tag}: {text}");
        }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_read_mac(out: *mut u8, _kind: i32) -> i32 {
    unsafe {
        let mac = esp_hal::efuse::base_mac_address();
        core::ptr::copy_nonoverlapping(mac.as_bytes().as_ptr(), out, 6);
        0
    }
}
