//! Link-down DHCP deadlines must not keep the priority1 executor runnable.
use core::{future::Future, task::Poll};
use embassy_net::{driver::Driver, Runner, Stack};
use embassy_time::Timer;

pub(super) async fn run<D: Driver>(stack: Stack<'_>, mut runner: Runner<'_, D>) -> ! {
    let mut running = core::pin::pin!(runner.run());
    loop {
        core::future::poll_fn(|cx| {
            // The runner itself publishes link state and clears a lost lease.
            // Waiting for Stack::wait_link_up before this poll would deadlock.
            let _ = running.as_mut().poll(cx);
            if stack.is_link_up() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        // embassy-net0.8 can self-wake continuously on an already due DHCP
        // deadline with link down. A real wait admits lower-priority services;
        // an Up transition is detected within 50ms. Connected traffic retains
        // the runner's normal event-driven cadence.
        Timer::after_millis(50).await;
    }
}
