//! The sensor task hands out the left slot only after descriptor detection.
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::{gpio::interconnect::InputSignal, peripherals::GPIO53};

pub struct Lease {
    pub reset: GPIO53<'static>,
    pub data: Option<[InputSignal<'static>; 4]>, // D3, D4, D5, D7
}
// All producers/consumers execute on core 0. The lease never crosses cores;
// the channels serialize the handoff of pins and frozen input routes.
unsafe impl Send for Lease {}
static REQUESTED: AtomicBool = AtomicBool::new(false);
static PRESENT: AtomicBool = AtomicBool::new(false);
static GRANTED: Channel<CriticalSectionRawMutex, Result<Lease, &'static str>, 1> = Channel::new();
static RETURNED: Channel<CriticalSectionRawMutex, Lease, 1> = Channel::new();

pub async fn acquire() -> Result<Lease, &'static str> {
    REQUESTED.store(true, Ordering::Release);
    GRANTED.receive().await
}
pub fn requested() -> bool {
    REQUESTED.swap(false, Ordering::AcqRel)
}
pub fn grant(result: Result<Lease, &'static str>) {
    assert!(GRANTED.try_send(result).is_ok());
}
pub fn release(lease: Lease) {
    assert!(RETURNED.try_send(lease).is_ok());
}
pub fn returned() -> Option<Lease> {
    RETURNED.try_receive().ok()
}
pub fn present() -> bool {
    PRESENT.load(Ordering::Acquire)
}
pub fn set_present(present: bool) {
    PRESENT.store(present, Ordering::Release);
}
