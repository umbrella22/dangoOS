//! Preemptible Core0 services. Synchronous storage/codec work never runs on
//! the executor that pumps HCI, IP packets, touch, or the power latch.
use alloc::{boxed::Box, sync::Arc};
use embassy_executor::SendSpawner;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use esp_hal::system::Cpu;

// Replaces the 8 KiB bt_lifecycle thread as well as hosting storage/encoding.
// Net internal stack cost is 8 KiB. The UI retains its own 24 KiB on Core1.
pub const STACK_BYTES: usize = 16 * 1024;
static READY: Signal<CriticalSectionRawMutex, SendSpawner> = Signal::new();
type Job = Box<dyn FnOnce() + Send>;
static JOBS: Channel<CriticalSectionRawMutex, Job, 4> = Channel::new();
// A queued close retains its open descriptor, so at most MAX_FILES can exist.
static CLOSES: Channel<CriticalSectionRawMutex, i32, { fs::vfs::MAX_FILES }> = Channel::new();

/// Owned request/result, with backpressure. Dropping a caller never frees data
/// the worker still uses, and never cancels an in-flight filesystem operation.
pub async fn run<R: Send + 'static>(job: impl FnOnce() -> R + Send + 'static) -> R {
    let done = Arc::new(Signal::<CriticalSectionRawMutex, R>::new());
    let reply = Arc::clone(&done);
    JOBS.send(Box::new(move || reply.signal(job()))).await;
    done.wait().await
}

pub fn close_later(fd: i32) {
    CLOSES
        .try_send(fd)
        .expect("queued closes cannot exceed open VFS descriptors");
}

#[embassy_executor::task]
async fn jobs() {
    loop {
        while let Ok(fd) = CLOSES.try_receive() {
            let _ = fs::global::with(|vfs| vfs.close(fd));
        }
        match embassy_futures::select::select(JOBS.receive(), embassy_time::Timer::after_millis(5))
            .await
        {
            embassy_futures::select::Either::First(job) => job(),
            embassy_futures::select::Either::Second(()) => (),
        }
    }
}

static VFS_LOCK: fs::global::LockHooks = fs::global::LockHooks {
    context_id: || {
        // RTOS threads on the same core can preempt one another. A core ID
        // would mistake a different thread for recursive VFS entry.
        let thread: usize;
        unsafe { core::arch::asm!("mv {0}, tp", out(reg) thread, options(nomem, nostack)) };
        assert_ne!(thread, 0, "VFS requires a running RTOS thread");
        thread
    },
    wait: || {
        let status: u32;
        unsafe { core::arch::asm!("csrr {0}, mstatus", out(reg) status, options(nomem, nostack)) };
        if !esp_hal::interrupt::RunLevel::current().is_thread() || status & 8 == 0 {
            return false;
        }
        esp_radio_rtos_driver::usleep(1_000);
        true
    },
};

pub async fn start() -> SendSpawner {
    fs::global::set_lock_hooks(&VFS_LOCK);
    // Priority 1 preempts priority 0 services. Native radio workers keep their
    // upstream priorities; the IRQ executors keep their hardware priorities.
    esp_rtos::CurrentThreadHandle::get().set_priority(1);
    esp_rtos::thread::ThreadSpawner::new(STACK_BYTES)
        .with_name("os_services")
        .with_priority(0)
        .with_pinned_to(Cpu::ProCpu)
        .spawn(|| {
            let executor = Box::leak(Box::new(esp_rtos::embassy::Executor::new()));
            executor.run(|spawner| {
                spawner.spawn(crate::tasks::radio::classic::host_task().unwrap());
                spawner.spawn(jobs().unwrap());
                READY.signal(spawner.make_send());
                log::info!(
                    "services: Core0 priority=0 stack={STACK_BYTES}; radio/input/power priority=1"
                );
            });
        })
        .detach();
    READY.wait().await
}
