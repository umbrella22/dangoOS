//! Blocking Bluedroid lifecycle calls cannot occupy the storage executor.
use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};
use esp_hal::system::Cpu;

static STARTED: AtomicBool = AtomicBool::new(false);
const STACK_BYTES: usize = 8 * 1024;

/// Allocate once, on first use. A stuck native call keeps this worker and its
/// controller alive; filesystem jobs, USB and UI can continue independently.
pub fn start() -> Result<(), &'static str> {
    if STARTED.load(Ordering::Acquire) {
        return Ok(());
    }
    let spawner = esp_rtos::thread::ThreadSpawner::try_new(STACK_BYTES)
        .map_err(|_| "native worker stack unavailable")?;
    spawner
        .with_name("bt_lifecycle")
        .with_priority(0)
        .with_pinned_to(Cpu::ProCpu)
        .spawn(|| {
            let executor = Box::leak(Box::new(esp_rtos::embassy::Executor::new()));
            executor.run(|spawner| {
                spawner.spawn(crate::services::radio::device::classic::host_task().unwrap());
            });
        })
        .detach();
    STARTED.store(true, Ordering::Release);
    Ok(())
}
