//! CDC connection handling and bounded-record transmission.
use super::logging::LOG_CHANNEL;

use super::{Driver, CONNECTED, REBOOT_ACK_SENT};
use core::sync::atomic::Ordering;
use embassy_futures::select::{select, Either};
use embassy_time::Timer;
use embassy_usb::{class::cdc_acm::Sender, driver::EndpointError};

/// Pump queued log lines to the CDC-ACM class.
///
/// Waits for the host to open the port, then drains the bounded channel.
/// Checks connection state while idle without producing console traffic.
pub async fn pump<'a>(class: &mut Sender<'a, Driver<'a>>) {
    loop {
        class.wait_connection().await;

        // Hold the queue until a host opens the port (DTR), so boot lines
        // are not sent into a port nobody reads.
        while !class.dtr() {
            Timer::after_millis(20).await;
        }

        CONNECTED.store(true, Ordering::Relaxed);
        loop {
            // Closing a CDC client drops DTR without disabling the USB endpoint.
            // Wake the bounded standby service on either form of disconnect.
            if !class.dtr() || !CONNECTED.load(Ordering::Relaxed) {
                CONNECTED.store(false, Ordering::Relaxed);
                crate::services::power::coordinator::wake();
                break;
            }

            let record = match select(LOG_CHANNEL.receive(), Timer::after_secs(5)).await {
                Either::First(record) => record,
                Either::Second(()) => continue,
            };
            let result = class
                .write_packet(&record.bytes[..record.len as usize])
                .await;
            if result.is_ok() && record.reboot_fence {
                REBOOT_ACK_SENT.store(true, Ordering::Release);
            }
            match result {
                Ok(()) => {}
                Err(EndpointError::Disabled) => {
                    // Cable removed or bus reset. Wait for the next connection;
                    // queued records stay bounded while disconnected.
                    CONNECTED.store(false, Ordering::Relaxed);
                    break;
                }
                Err(EndpointError::BufferOverflow) => {
                    // A single record exceeded the endpoint packet size. Drop it
                    // rather than treating it as a disconnect.
                }
            }
        }
    }
}
