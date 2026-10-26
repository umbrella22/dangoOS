//! Haptic patterns on the vibration motor (`GPIO8`, active high).
//!
//! ## Pattern source
//!
//! The table is RFC 0004 section 6. Pattern selection and the 150 ms rate
//! limit are the scheduler's job; the affect engine only names a
//! [`HapticPattern`] in its `AffectTick`.
//!
//! | Reaction/Event | Bursts | Intensity |
//! | --- | --- | --- |
//! | `Panic` | 3 x 80 ms, 60 ms gaps | 100% |
//! | `Angry` | 2 x 160 ms, 100 ms gap | 90% |
//! | `Surprised` | 1 x 50 ms | 70% |
//! | `Puzzled` | 2 x 40 ms, 50 ms gap | 60% |
//! | `Happy` | 1 x 60 ms | 50% |
//! | `Satisfied` | 1 x 70 ms | 55% |
//! | `Confused` | 2 x 35 ms, 60 ms gap | 45% |
//! | `Listening` | 1 x 50 ms | 60% |
//! | `Thinking` | 1 x 30 ms | 35% |
//! | `Error` | 3 x 60 ms, 40 ms gaps | 100% |
//! | Module insert | 1 x 40 ms | 45% |
//! | Module remove | 2 x 40 ms, 50 ms gap | 45% |
//! | Magnetic contact | 1 x 30 ms | 35% |
//!
//! ## PWM backend
//!
//! The official BSP drives `GPIO8` with ESP-IDF LEDC (`BSP_MOTOR_GPIO`,
//! `CONFIG_BSP_MOTOR_ENABLE_PWM`, 5 kHz). The esp-hal revision pinned by this
//! workspace marks the S31 LEDC and MCPWM drivers as unsupported, so this
//! module produces the same 5 kHz PWM with the RMT TX peripheral instead:
//!
//! - The shared RMT runs at 10 MHz; this channel divides by 10 for a 1 us tick.
//! - One [`PulseCode`] encodes a 200-tick period (5 kHz) with the pattern's
//!   duty cycle.
//! - `transmit_continuously(..., LoopMode::Infinite)` replays that period in
//!   hardware while the task sleeps for the burst duration.
//! - `stop()` returns the pin to its idle-low level between bursts.
//!
//! The task wakes at burst/gap deadlines or request/cancellation notifications,
//! never per PWM edge. If the RMT channel cannot be set up,
//! `main` holds `GPIO8` low and haptics are disabled; no other task blocks.
//!
//! ## Priority and rate limit
//!
//! [`play`] replaces the pending pattern when the new priority is at least the
//! pending one. `Error` (3) > `Panic` (2) > everything else (1). A running
//! pattern is stopped early when a strictly higher-priority request
//! arrives. Pattern starts are rate-limited to one per [`RATE_LIMIT_MS`]
//! (150 ms); a request that arrives inside the window waits for it.

#[cfg(feature = "haptic-diagnostics")]
mod diagnostics;
mod policy;
use policy::{Policy, Request};

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
pub fn set_enabled(enabled: bool) {
    critical_section::with(|cs| POLICY.borrow_ref_mut(cs).set_enabled(enabled));
    // Notification only: every waiter also checks the policy epoch/state.
    WAKE.signal(());
}
pub fn enabled() -> bool {
    critical_section::with(|cs| POLICY.borrow_ref(cs).enabled())
}
/// Disable admission and wait for the task to acknowledge actual PWM stop.
/// A failed peripheral stop deliberately never produces a silence receipt;
/// callers must bound this wait and refuse the operation on timeout.
pub async fn quiesce() {
    set_enabled(false);
    loop {
        if critical_section::with(|cs| POLICY.borrow_ref(cs).quiescent()) {
            return;
        }
        Timer::after_millis(1).await;
    }
}
static READY: AtomicBool = AtomicBool::new(false);
pub fn available() -> bool {
    READY.load(Ordering::Acquire)
}

use critical_section::Mutex as CsMutex;
use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    gpio::Level,
    rmt::{
        Channel, ContinuousTxTransaction, Error as RmtError, LoopMode, PulseCode, Tx,
        TxChannelConfig,
    },
    Blocking,
};
use log::{info, warn};
use os::behavior::affect::HapticPattern;

/// RMT TX channel configured in `main` and owned by the haptic task.
pub type HapticChannel = Channel<'static, Blocking, Tx>;

/// One pattern per 150 ms (RFC 0004 section 6).
pub const RATE_LIMIT_MS: u64 = 150;

/// 5 kHz PWM (the BSP motor configuration) at the RMT's 1 MHz tick.
const PWM_PERIOD_TICKS: u16 = 200;

/// Pending request slot. `play` is callable from any task.
static POLICY: CsMutex<RefCell<Policy<HapticPattern>>> = CsMutex::new(RefCell::new(Policy::new()));
/// Wakes the haptic task when a request is posted.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Request one pattern. Non-blocking and safe to call from the sensor/affect
/// task. Returns whether admission succeeded. Disabled/faulted output and a
/// higher-priority pending pattern reject the request.
///
/// The UI task calls this with each new `AffectTick::haptic`.
pub fn play(pattern: HapticPattern) -> bool {
    let accepted =
        critical_section::with(|cs| POLICY.borrow_ref_mut(cs).post(pattern, pattern.priority()));
    if accepted {
        WAKE.signal(());
    }
    accepted
}

/// Burst shape for one pattern.
#[derive(Clone, Copy, Debug)]
struct PatternSpec {
    bursts: u8,
    on_ms: u16,
    gap_ms: u16,
    duty_pct: u8,
}

/// RFC 0004 section 6 pattern table.
const fn spec(pattern: HapticPattern) -> PatternSpec {
    match pattern {
        HapticPattern::Panic => PatternSpec {
            bursts: 3,
            on_ms: 80,
            gap_ms: 60,
            duty_pct: 100,
        },
        HapticPattern::Angry => PatternSpec {
            bursts: 2,
            on_ms: 160,
            gap_ms: 100,
            duty_pct: 90,
        },
        HapticPattern::Surprised => PatternSpec {
            bursts: 1,
            on_ms: 50,
            gap_ms: 0,
            duty_pct: 70,
        },
        HapticPattern::Puzzled => PatternSpec {
            bursts: 2,
            on_ms: 40,
            gap_ms: 50,
            duty_pct: 60,
        },
        HapticPattern::Happy => PatternSpec {
            bursts: 1,
            on_ms: 60,
            gap_ms: 0,
            duty_pct: 50,
        },
        HapticPattern::Satisfied => PatternSpec {
            bursts: 1,
            on_ms: 70,
            gap_ms: 0,
            duty_pct: 55,
        },
        HapticPattern::Confused => PatternSpec {
            bursts: 2,
            on_ms: 35,
            gap_ms: 60,
            duty_pct: 45,
        },
        HapticPattern::Listening => PatternSpec {
            bursts: 1,
            on_ms: 50,
            gap_ms: 0,
            duty_pct: 60,
        },
        HapticPattern::Thinking => PatternSpec {
            bursts: 1,
            on_ms: 30,
            gap_ms: 0,
            duty_pct: 35,
        },
        HapticPattern::Error => PatternSpec {
            bursts: 3,
            on_ms: 60,
            gap_ms: 40,
            duty_pct: 100,
        },
        HapticPattern::ModuleInsert => PatternSpec {
            bursts: 1,
            on_ms: 40,
            gap_ms: 0,
            duty_pct: 45,
        },
        HapticPattern::ModuleRemove => PatternSpec {
            bursts: 2,
            on_ms: 40,
            gap_ms: 50,
            duty_pct: 45,
        },
        HapticPattern::MagneticContact => PatternSpec {
            bursts: 1,
            on_ms: 30,
            gap_ms: 0,
            duty_pct: 35,
        },
    }
}

/// Haptic scheduler task. `channel` is created in `main` so no other task
/// touches the RMT peripheral.
#[embassy_executor::task]
pub async fn haptic_task(channel: HapticChannel) {
    READY.store(true, Ordering::Release);
    info!("haptic_task: GPIO8 5 kHz RMT PWM, rate limit {RATE_LIMIT_MS} ms");
    let mut channel = Some(channel);
    let mut last_start_ms: Option<u64> = None;
    let mut next_request: Option<Request<HapticPattern>> = None;

    loop {
        let mut pattern = match next_request.take() {
            Some(pattern) => pattern,
            None => wait_for_request().await,
        };

        if !wait_for_rate_limit(last_start_ms, pattern).await {
            finish_request();
            continue;
        }

        // A request posted while waiting for the rate limit replaces the
        // selected pattern when its priority is at least as high.
        if let Some(pending) = peek_pending() {
            if pending.priority >= pattern.priority {
                pattern = take_pending().unwrap_or(pattern);
            }
        }

        if valid(pattern) {
            last_start_ms = Some(now_ms());
            next_request = run_pattern(&mut channel, pattern).await;
        }
        if next_request.is_none() {
            finish_request();
        }
    }
}

/// Block until a request exists. `play` signals [`WAKE`] after storing it.
async fn wait_for_request() -> Request<HapticPattern> {
    loop {
        if let Some(pattern) = take_pending() {
            return pattern;
        }
        WAKE.wait().await;
    }
}

fn take_pending() -> Option<Request<HapticPattern>> {
    critical_section::with(|cs| POLICY.borrow_ref_mut(cs).take())
}
fn peek_pending() -> Option<Request<HapticPattern>> {
    critical_section::with(|cs| POLICY.borrow_ref(cs).peek())
}
fn valid(request: Request<HapticPattern>) -> bool {
    critical_section::with(|cs| POLICY.borrow_ref(cs).valid(request))
}
fn finish_request() {
    critical_section::with(|cs| POLICY.borrow_ref_mut(cs).finish());
}
fn peripheral_failed() {
    READY.store(false, Ordering::Release);
    critical_section::with(|cs| POLICY.borrow_ref_mut(cs).failed());
}
fn take_if_higher(current: u8) -> Option<Request<HapticPattern>> {
    critical_section::with(|cs| {
        let mut policy = POLICY.borrow_ref_mut(cs);
        if policy.peek().is_some_and(|p| p.priority > current) {
            policy.take()
        } else {
            None
        }
    })
}

/// Hold the next pattern start until 150 ms after the previous one.
async fn wait_for_rate_limit(last_start_ms: Option<u64>, request: Request<HapticPattern>) -> bool {
    if !valid(request) {
        return false;
    }
    let Some(last_start_ms) = last_start_ms else {
        return valid(request);
    };
    let elapsed = now_ms().saturating_sub(last_start_ms);
    if elapsed >= RATE_LIMIT_MS {
        return valid(request);
    }
    let deadline = Instant::now() + Duration::from_millis(RATE_LIMIT_MS - elapsed);
    loop {
        if !valid(request) {
            return false;
        }
        if Instant::now() >= deadline {
            return valid(request);
        }
        // Consume wake-ups so a new request can replace the pending slot while
        // the rate limit runs, but do not start before the deadline.
        let _ = select(Timer::at(deadline), WAKE.wait()).await;
    }
}

/// Play one pattern, returning a higher-priority request that arrived while it
/// was running. Every started transaction attempts stop before returning;
/// a stop error faults the backend and prevents a quiescence receipt.
async fn run_pattern(
    channel: &mut Option<HapticChannel>,
    request: Request<HapticPattern>,
) -> Option<Request<HapticPattern>> {
    let pattern = request.pattern;
    #[cfg(feature = "haptic-diagnostics")]
    let diagnostic_pattern = diagnostics::next_pattern();
    let spec = spec(pattern);
    info!(
        "haptic: {:?} ({} burst(s) x {} ms, {}%)",
        pattern, spec.bursts, spec.on_ms, spec.duty_pct
    );

    for burst in 0..spec.bursts {
        if !valid(request) {
            return None;
        }
        if burst > 0 && spec.gap_ms > 0 {
            if let Some(next) = wait_or_preempt(spec.gap_ms, request).await {
                return Some(next);
            }
        }

        let Some(channel_handle) = channel.take() else {
            return None;
        };
        // Serialize the final admission check with disable, so a request cannot
        // start a new burst after its epoch was invalidated.
        let started = critical_section::with(|cs| {
            if POLICY.borrow_ref(cs).valid(request) {
                Some(start_pulse(channel_handle, spec.duty_pct))
            } else {
                *channel = Some(channel_handle);
                None
            }
        });
        let transaction = match started? {
            Ok(transaction) => transaction,
            Err((error, channel_handle)) => {
                warn!("haptic: PWM start failed: {error:?}");
                peripheral_failed();
                *channel = Some(channel_handle);
                return None;
            }
        };

        #[cfg(feature = "haptic-diagnostics")]
        if burst == 0 {
            diagnostics::record(diagnostic_pattern, burst + 1, "start");
        }
        let preempting = wait_or_preempt(spec.on_ms, request).await;
        #[cfg(feature = "haptic-diagnostics")]
        if burst == 0 {
            diagnostics::record(diagnostic_pattern, burst + 1, "prestop");
        }
        match transaction.stop() {
            Ok(channel_handle) => *channel = Some(channel_handle),
            Err((error, channel_handle)) => {
                warn!("haptic: PWM stop failed: {error:?}");
                peripheral_failed();
                *channel = Some(channel_handle);
            }
        }
        #[cfg(feature = "haptic-diagnostics")]
        if burst == 0 {
            diagnostics::record(diagnostic_pattern, burst + 1, "stop");
        }
        if let Some(next) = preempting {
            return Some(next);
        }
    }
    None
}

/// Start one burst: continuous high at 100%, otherwise 5 kHz PWM.
fn start_pulse(
    channel: HapticChannel,
    duty_pct: u8,
) -> Result<ContinuousTxTransaction<'static>, (RmtError, HapticChannel)> {
    let duty = duty_pct.min(100) as u32;
    let pulse = if duty == 100 {
        // Two nonzero high segments represent full duty without a zero-length
        // end marker or the legacy 1-tick low interval.
        PulseCode::new(Level::High, 100, Level::High, 100)
    } else {
        let high = ((PWM_PERIOD_TICKS as u32 * duty) / 100).clamp(1, PWM_PERIOD_TICKS as u32 - 1);
        let low = PWM_PERIOD_TICKS as u32 - high;
        PulseCode::new(Level::High, high as u16, Level::Low, low as u16)
    };
    // `transmit_continuously` requires the sequence to end with an end marker.
    // Continuous TX restarts the sequence via TX_CONTI_MODE; it does not use
    // memory-wrap mode. Keep the marker HAL requires; stop() ends the burst.
    let sequence = [pulse, PulseCode::end_marker()];
    channel.transmit_continuously(&sequence, LoopMode::Infinite)
}

/// Sleep for `duration_ms`, returning early on cancellation or when a strictly
/// higher-priority pattern is requested.
async fn wait_or_preempt(
    duration_ms: u16,
    request: Request<HapticPattern>,
) -> Option<Request<HapticPattern>> {
    if duration_ms == 0 {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(duration_ms as u64);
    loop {
        if !valid(request) {
            return None;
        }
        if Instant::now() >= deadline {
            return None;
        }
        match select(Timer::at(deadline), WAKE.wait()).await {
            Either::First(()) => return None,
            Either::Second(()) => {
                if let Some(next) = take_if_higher(request.priority) {
                    return Some(next);
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis()
}

/// The RMT peripheral runs at 10 MHz for the WS2812 light (`lights`); the
/// haptic channel divides it back to the 1 MHz tick its periods assume.
const RMT_TICKS_PER_US: u8 = 10;

/// Configure the RMT TX channel that the haptic task will own.
///
/// The caller attaches `GPIO8` with [`Channel::with_pin`] so a setup failure
/// can still leave the pin driven low.
pub fn configure_channel(
    creator: esp_hal::rmt::ChannelCreator<'static, Blocking, 0>,
) -> Result<HapticChannel, esp_hal::rmt::ConfigError> {
    use esp_hal::rmt::TxChannelCreator;

    let config = TxChannelConfig::default()
        .with_clk_divider(RMT_TICKS_PER_US)
        .with_idle_output(true)
        .with_idle_output_level(Level::Low);
    creator.configure_tx(&config)
}
