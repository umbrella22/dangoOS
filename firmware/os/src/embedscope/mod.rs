//! Optional device diagnostics. Native reads stay on the UI thread; USB and
//! display tasks exchange bounded, owned messages.
#[cfg(any(target_os = "none", test))]
mod capture;
#[cfg(target_os = "none")]
mod device;
#[cfg(any(target_os = "none", test))]
mod frame;
#[cfg(target_os = "none")]
mod logs;
#[cfg(any(target_os = "none", test))]
pub(crate) mod panic_record;
#[cfg(target_os = "none")]
pub(crate) mod transport;
#[cfg(any(target_os = "none", test))]
mod tree;
#[cfg(target_os = "none")]
pub(crate) use capture::FinalCapture;
#[cfg(target_os = "none")]
pub(crate) use device::UiAgent;
#[cfg(target_os = "none")]
pub(crate) use logs::record;

/// Allocate diagnostics queues after the general heap is ready, before either
/// USB or the second core can access them. They need no fixed internal BSS.
#[cfg(target_os = "none")]
pub(crate) fn init() {
    transport::init();
    logs::init();
}
