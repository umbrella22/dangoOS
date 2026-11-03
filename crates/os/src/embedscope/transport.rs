//! Independent second CDC. Epochs fence every queued frame on DTR/reset.
use alloc::boxed::Box;
use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::{
    join::join3,
    select::{select, Either},
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, once_lock::OnceLock,
};
use embassy_time::Timer;
use embassy_usb::class::cdc_acm::{CdcAcmClass, ControlChanged, Receiver, Sender};
use escope_protocol::{FrameDecoder, MAX_FRAME};
use esp_hal::usb::otg::embassy_usb_device::Driver;

static CONNECTED: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU32 = AtomicU32::new(1);
pub(super) struct WireFrame {
    pub epoch: u32,
    pub len: usize,
    pub bytes: [u8; MAX_FRAME],
}
type FrameQueue = Channel<CriticalSectionRawMutex, WireFrame, 4>;
static RX: OnceLock<&'static FrameQueue> = OnceLock::new();
static TX: OnceLock<&'static FrameQueue> = OnceLock::new();
pub(super) fn init() {
    assert!(RX.init(Box::leak(Box::new(FrameQueue::new()))).is_ok());
    assert!(TX.init(Box::leak(Box::new(FrameQueue::new()))).is_ok());
}
pub(super) fn requests() -> &'static FrameQueue {
    RX.try_get().expect("EmbedScope queues initialized at boot")
}
pub(super) fn responses() -> &'static FrameQueue {
    TX.try_get().expect("EmbedScope queues initialized at boot")
}
pub(crate) fn connected() -> bool {
    CONNECTED.load(Ordering::Acquire)
}
pub(super) fn epoch() -> u32 {
    EPOCH.load(Ordering::Acquire)
}
fn connection(value: bool) {
    if CONNECTED.swap(value, Ordering::AcqRel) != value {
        EPOCH.fetch_add(1, Ordering::AcqRel);
    }
}
/// Construct the large transport future outside the USB task's poll frame.
/// Inlining this allocation retains its temporary space while storage mounts.
#[inline(never)]
pub(crate) fn run(
    class: CdcAcmClass<'static, Driver<'static>>,
) -> Pin<Box<impl Future<Output = ()>>> {
    Box::pin(run_inner(class))
}
async fn run_inner(class: CdcAcmClass<'static, Driver<'static>>) {
    let (mut tx, mut rx, control) = class.split_with_control();
    join3(receive(&mut rx), transmit(&mut tx), watch(control)).await;
}
async fn watch(control: ControlChanged<'static>) {
    loop {
        connection(control.dtr());
        control.control_changed().await;
    }
}
async fn receive(rx: &mut Receiver<'static, Driver<'static>>) {
    let mut decoder = FrameDecoder::new();
    let mut current = epoch();
    let mut packet = [0; 512];
    loop {
        rx.wait_connection().await;
        if !connected() || !rx.dtr() {
            Timer::after_millis(5).await;
            continue;
        }
        if current != epoch() {
            decoder = FrameDecoder::new();
            current = epoch();
        }
        let count = match select(rx.read_packet(&mut packet), Timer::after_millis(50)).await {
            Either::First(Ok(n)) => n,
            Either::First(Err(_)) => {
                connection(false);
                continue;
            }
            Either::Second(()) => continue,
        };
        for &byte in &packet[..count] {
            if let Some(Ok(frame)) = decoder.push(byte) {
                let mut item = WireFrame {
                    epoch: current,
                    len: frame.len(),
                    bytes: [0; MAX_FRAME],
                };
                item.bytes[..frame.len()].copy_from_slice(frame);
                // USB backpressure is bounded and cancellable; the UI never waits.
                loop {
                    if !connected() || epoch() != current {
                        break;
                    }
                    match requests().try_send(item) {
                        Ok(()) => break,
                        Err(embassy_sync::channel::TrySendError::Full(value)) => {
                            item = value;
                            Timer::after_millis(1).await;
                        }
                    }
                }
            }
        }
    }
}
async fn transmit(tx: &mut Sender<'static, Driver<'static>>) {
    loop {
        let frame = responses().receive().await;
        if frame.epoch != epoch() || !connected() {
            continue;
        }
        tx.wait_connection().await;
        let mut sent = 0;
        for chunk in frame.bytes[..frame.len].chunks(512) {
            if frame.epoch != epoch() || !connected() || !tx.dtr() {
                break;
            }
            match select(tx.write_packet(chunk), Timer::after_millis(250)).await {
                Either::First(Ok(())) => {
                    sent += chunk.len();
                }
                _ => {
                    connection(false);
                    break;
                }
            }
        }
        if sent == frame.len
            && frame.len % 512 == 0
            && frame.epoch == epoch()
            && connected()
            && tx.dtr()
        {
            if !matches!(
                select(tx.write_packet(&[]), Timer::after_millis(250)).await,
                Either::First(Ok(()))
            ) {
                connection(false);
            }
        }
    }
}
