//! H1.3 haptic patterns on the vibration motor (`GPIO8`, active high).
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
//! - `Rmt::new(RMT, 1 MHz)` gives a 1 us tick.
//! - One [`PulseCode`] encodes a 200-tick period (5 kHz) with the pattern's
//!   duty cycle.
//! - `transmit_continuously(..., LoopMode::Infinite)` replays that period in
//!   hardware while the task sleeps for the burst duration.
//! - `stop()` returns the pin to its idle-low level between bursts.
//!
//! The task only wakes at burst/gap boundaries, never per PWM edge, so it
//! cannot starve the UI or sensor tasks. If the RMT channel cannot be set up,
//! `main` holds `GPIO8` low and haptics are disabled; no other task blocks.
//!
//! ## Priority and rate limit
//!
//! [`play`] replaces the pending pattern when the new priority is at least the
//! pending one. `Error` (3) > `Panic` (2) > everything else (1). A running
//! pattern is preempted between bursts when a strictly higher-priority request
//! arrives. Pattern starts are rate-limited to one per [`RATE_LIMIT_MS`]
//! (150 ms); a request that arrives inside the window waits for it.

use core::cell::RefCell;

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
use grok_ball::affect::HapticPattern;
use log::{info, warn};

/// RMT TX channel configured in `main` and owned by the haptic task.
pub type HapticChannel = Channel<'static, Blocking, Tx>;

/// One pattern per 150 ms (RFC 0004 section 6).
pub const RATE_LIMIT_MS: u64 = 150;

/// 5 kHz PWM (the BSP motor configuration) at the RMT's 1 MHz tick.
const PWM_PERIOD_TICKS: u16 = 200;

/// Pending request slot. `play` is callable from any task.
static PENDING: CsMutex<RefCell<Option<HapticPattern>>> = CsMutex::new(RefCell::new(None));
/// Wakes the haptic task when a request is posted.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Request one pattern. Non-blocking and safe to call from the sensor/affect
/// task. Returns whether the request was accepted (it is ignored only when a
/// higher-priority pattern is already pending).
///
/// H2's affect integration calls this with `AffectTick::haptic`; nothing in the
/// current firmware calls it yet, so it is intentionally unused here.
#[allow(dead_code)]
pub fn play(pattern: HapticPattern) -> bool {
    let accepted = critical_section::with(|cs| {
        let mut pending = PENDING.borrow_ref_mut(cs);
        let replace = match *pending {
            None => true,
            Some(current) => priority(pattern) >= priority(current),
        };
        if replace {
            *pending = Some(pattern);
        }
        replace
    });
    if accepted {
        WAKE.signal(());
    }
    accepted
}

/// Priority ordering from RFC 0004 section 6: Error > Panic > all others.
pub const fn priority(pattern: HapticPattern) -> u8 {
    match pattern {
        HapticPattern::Error => 3,
        HapticPattern::Panic => 2,
        _ => 1,
    }
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
    info!("haptic_task: GPIO8 5 kHz RMT PWM, rate limit {RATE_LIMIT_MS} ms");
    let mut channel = Some(channel);
    let mut last_start_ms: Option<u64> = None;
    let mut next_request: Option<HapticPattern> = None;

    loop {
        let mut pattern = match next_request.take() {
            Some(pattern) => pattern,
            None => wait_for_request().await,
        };

        wait_for_rate_limit(last_start_ms).await;

        // A request posted while waiting for the rate limit replaces the
        // selected pattern when its priority is at least as high.
        if let Some(pending) = peek_pending() {
            if priority(pending) >= priority(pattern) {
                pattern = take_pending().unwrap_or(pattern);
            }
        }

        last_start_ms = Some(now_ms());
        if let Some(preempting) = run_pattern(&mut channel, pattern).await {
            next_request = Some(preempting);
        }
    }
}

/// Block until a request exists. `play` signals [`WAKE`] after storing it.
async fn wait_for_request() -> HapticPattern {
    loop {
        if let Some(pattern) = take_pending() {
            return pattern;
        }
        WAKE.wait().await;
    }
}

fn take_pending() -> Option<HapticPattern> {
    critical_section::with(|cs| PENDING.borrow_ref_mut(cs).take())
}

fn peek_pending() -> Option<HapticPattern> {
    critical_section::with(|cs| *PENDING.borrow_ref(cs))
}

/// Take the pending request only when it outranks `current`.
fn take_if_higher(current: u8) -> Option<HapticPattern> {
    critical_section::with(|cs| {
        let mut pending = PENDING.borrow_ref_mut(cs);
        match *pending {
            Some(pattern) if priority(pattern) > current => {
                *pending = None;
                Some(pattern)
            }
            _ => None,
        }
    })
}

/// Hold the next pattern start until 150 ms after the previous one.
async fn wait_for_rate_limit(last_start_ms: Option<u64>) {
    let Some(last_start_ms) = last_start_ms else {
        return;
    };
    let elapsed = now_ms().saturating_sub(last_start_ms);
    if elapsed >= RATE_LIMIT_MS {
        return;
    }
    let deadline = Instant::now() + Duration::from_millis(RATE_LIMIT_MS - elapsed);
    loop {
        if Instant::now() >= deadline {
            return;
        }
        // Consume wake-ups so a new request can replace the pending slot while
        // the rate limit runs, but do not start before the deadline.
        let _ = select(Timer::at(deadline), WAKE.wait()).await;
    }
}

/// Play one pattern, returning a higher-priority request that arrived while it
/// was running. The motor is always stopped before returning.
async fn run_pattern(
    channel: &mut Option<HapticChannel>,
    pattern: HapticPattern,
) -> Option<HapticPattern> {
    let spec = spec(pattern);
    let pattern_priority = priority(pattern);
    info!(
        "haptic: {:?} ({} burst(s) x {} ms, {}%)",
        pattern, spec.bursts, spec.on_ms, spec.duty_pct
    );

    for burst in 0..spec.bursts {
        if burst > 0 && spec.gap_ms > 0 {
            if let Some(next) = wait_or_preempt(spec.gap_ms, pattern_priority).await {
                return Some(next);
            }
        }

        let Some(channel_handle) = channel.take() else {
            return None;
        };
        let transaction = match start_pulse(channel_handle, spec.duty_pct) {
            Ok(transaction) => transaction,
            Err((error, channel_handle)) => {
                warn!("haptic: PWM start failed: {error:?}");
                *channel = Some(channel_handle);
                return None;
            }
        };

        let preempting = wait_or_preempt(spec.on_ms, pattern_priority).await;
        match transaction.stop() {
            Ok(channel_handle) => *channel = Some(channel_handle),
            Err((error, channel_handle)) => {
                warn!("haptic: PWM stop failed: {error:?}");
                *channel = Some(channel_handle);
            }
        }
        if let Some(next) = preempting {
            return Some(next);
        }
    }
    None
}

/// Start the continuous 5 kHz PWM that represents one burst.
fn start_pulse(
    channel: HapticChannel,
    duty_pct: u8,
) -> Result<ContinuousTxTransaction<'static>, (RmtError, HapticChannel)> {
    let duty = duty_pct.min(100) as u32;
    let high = ((PWM_PERIOD_TICKS as u32 * duty) / 100).clamp(1, PWM_PERIOD_TICKS as u32 - 1);
    let low = PWM_PERIOD_TICKS as u32 - high;
    // `transmit_continuously` requires the sequence to end with an end marker.
    // In continuous mode the hardware wraps the memory block, so the marker is
    // only an API terminator; the PWM period repeats until `stop()`.
    let sequence = [
        PulseCode::new(Level::High, high as u16, Level::Low, low as u16),
        PulseCode::end_marker(),
    ];
    channel.transmit_continuously(&sequence, LoopMode::Infinite)
}

/// Sleep for `duration_ms`, returning early when a strictly higher-priority
/// pattern is requested.
async fn wait_or_preempt(duration_ms: u16, current_priority: u8) -> Option<HapticPattern> {
    if duration_ms == 0 {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(duration_ms as u64);
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match select(Timer::at(deadline), WAKE.wait()).await {
            Either::First(()) => return None,
            Either::Second(()) => {
                if let Some(next) = take_if_higher(current_priority) {
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

/// Configure the RMT TX channel that the haptic task will own.
///
/// The caller attaches `GPIO8` with [`Channel::with_pin`] so a setup failure
/// can still leave the pin driven low.
pub fn configure_channel(
    rmt: esp_hal::rmt::Rmt<'static, Blocking>,
) -> Result<HapticChannel, esp_hal::rmt::ConfigError> {
    use esp_hal::rmt::TxChannelCreator;

    let config = TxChannelConfig::default()
        .with_clk_divider(1)
        .with_idle_output(true)
        .with_idle_output_level(Level::Low);
    rmt.channel0.configure_tx(&config)
}
