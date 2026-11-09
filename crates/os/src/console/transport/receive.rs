//! CDC receive framing, packet tails and connection reset cleanup.

use super::{effects, Driver};
use crate::apps::device as apps;
use crate::console::protocol::Session;
use alloc::vec::Vec;
use embassy_futures::select::{select, Either};
use embassy_time::Timer;
use embassy_usb::{class::cdc_acm::Receiver, driver::EndpointError};

/// Feed host bytes to the app push protocol and carry out its effects.
///
/// File writes happen inside `feed`, synchronously; meanwhile the USB core
/// NAKs further OUT packets, which is the flow control.
pub async fn receive<'a>(class: &mut Receiver<'a, Driver<'a>>) {
    let mut buf = [0u8; 512];
    let mut session = Session::new();
    let mut pending_packet = None;
    loop {
        class.wait_connection().await;

        while !class.dtr() {
            Timer::after_millis(20).await;
        }
        loop {
            // Closing a CDC client does not disable the USB OUT endpoint.
            // Observe DTR even when no further packet arrives, so a partial
            // file cannot consume commands from the next client as its body.
            if !class.dtr() {
                break;
            }

            let result = match pending_packet.take() {
                Some(n) => Ok(n),
                None => match select(class.read_packet(&mut buf), Timer::after_millis(20)).await {
                    Either::First(result) => result,
                    Either::Second(()) => continue,
                },
            };
            let n = match result {
                Ok(n) => n,
                Err(EndpointError::Disabled) => {
                    break;
                }
                Err(EndpointError::BufferOverflow) => {
                    continue;
                }
            };
            // Preserve whole USB payload chunks, stopping only at a deferred
            // command boundary before processing the packet's remaining bytes.
            let mut consumed = 0;
            while consumed < n {
                session.set_update_active(
                    crate::services::update::boot::device::active()
                        || crate::services::update::task::busy(),
                );
                session.set_management_busy(apps::is_managing());
                let mut effects = Vec::new();

                // CDC state stays on one executor. Move only owned protocol
                // bytes/state to the storage worker; a blocking VFS operation
                // must not stop USB control, input, or network pumping.
                let packet = buf;
                let (next, fed, pending) =
                    crate::platform::runtime::executors::services::run(move || {
                        let fed = ::fs::global::with(|vfs| {
                            Ok(
                                session.feed_until_deferred(vfs, &packet[consumed..n], &mut |e| {
                                    effects.push(e)
                                }),
                            )
                        });
                        let fed = fed.unwrap_or_else(|_| {
                            session.feed_until_deferred(
                                &mut ::fs::Vfs::new(),
                                &packet[consumed..n],
                                &mut |e| effects.push(e),
                            )
                        });
                        (session, fed, effects)
                    })
                    .await;
                session = next;
                consumed += fed;

                effects::apply(pending).await;
            }
            // A continuously ready USB read does not yield to other thread tasks.
            embassy_futures::yield_now().await;
        }
        super::CONNECTED.store(false, core::sync::atomic::Ordering::Relaxed);
        if !class.dtr() {
            // CDC close leaves already queued OUT packets enabled. Drain them
            // before resetting the parser, so old file bytes cannot poison the
            // next client's control lines. Preserve its first packet if it
            // opens while this drain is waiting for an idle endpoint.
            loop {
                match select(class.read_packet(&mut buf), Timer::after_millis(20)).await {
                    Either::First(Ok(n)) => {
                        if class.dtr() {
                            pending_packet = Some(n);
                            break;
                        }
                    }
                    _ => break,
                }
            }
        }
        crate::services::power::coordinator::wake();
        // The host went away mid-push: drop the staged files.

        session = crate::platform::runtime::executors::services::run(move || {
            let _ = ::fs::global::with(|vfs| {
                session.reset(vfs);
                Ok(())
            });
            session
        })
        .await;
    }
}
