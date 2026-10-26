//! Interaction board WS2812 mood light.
//!
//! [`os::behavior::mood_light::MoodLight`] decides the colours; this
//! task sends them to the six LEDs every [`FRAME_MS`].
//!
//! - Data pin: right slot `GPIO38` (BSP `ws2812_io`, mirrored from the left
//!   slot's `GPIO15`). Left-slot light output is not implemented; `GPIO15`
//!   is also the camera's D1, and this channel stays attached to the right slot.
//! - Timing: RMT channel 1 at a 10 MHz tick (0.1 us), WS2812 800 kHz
//!   (0: 0.4/0.8 us, 1: 0.8/0.4 us), GRB order, MSB first. The frame gap is
//!   far longer than the 280 us latch time.
//! - Channel 1 takes three RMT memory blocks (144 codes) so one frame fits
//!   with a single refill for the end marker.
//! - Other tasks only touch atomics: [`show`], [`set_ambient`], [`spark`] and
//!   [`set_attached`]. Nothing here blocks them.
//! - Colour frames require an active right board. Startup and terminal
//!   shutdown clear GPIO38 even when discovery reports no board.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    gpio::Level,
    rmt::{Channel, ChannelCreator, PulseCode, Tx, TxChannelConfig, TxChannelCreator},
    Blocking,
};
use log::{info, warn};
use os::{
    behavior::mood_light::{preview_frame, Frame, MoodLight, Rgb, Side, LED_COUNT},
    Expression,
};

mod shutdown;
use shutdown::Lifecycle;

/// RMT TX channel 1, attached to `GPIO38` in `main`.
pub type LightChannel = Channel<'static, Blocking, Tx>;

/// Frame period (50 Hz).
pub const FRAME_MS: u64 = 20;
/// Resend an unchanged frame this often, in case a bit was lost.
const REFRESH_MS: u64 = 1000;
/// Six LEDs take about 173 us on the wire. Bound polling if RMT never ends.
const TX_TIMEOUT_US: u64 = 2_000;
/// Minimum low interval after the final WS2812 data bit before clear receipt.
const LATCH_US: u64 = 280;
static LIFECYCLE: critical_section::Mutex<core::cell::RefCell<Lifecycle>> =
    critical_section::Mutex::new(core::cell::RefCell::new(Lifecycle::new()));
fn shutting_down() -> bool {
    critical_section::with(|cs| LIFECYCLE.borrow_ref(cs).shutting_down())
}
/// Terminal from the same first AI-long latch as the power state machine.
pub fn begin_shutdown() {
    critical_section::with(|cs| {
        LIFECYCLE.borrow_ref_mut(cs).request_shutdown();
        *LUA_FRAME.borrow_ref_mut(cs) = None;
        *BRAIN_FRAME.borrow_ref_mut(cs) = None;
        REFLEX_UNTIL.borrow(cs).set(0);
    });
    OWNER.store(Owner::Dark as u8, Ordering::Release);
    SPARKS.store(0, Ordering::Relaxed);
    info!("lights: shutdown latched; fresh black TX required");
}
/// True only after this shutdown's successful black TX and latch interval.
pub fn shutdown_cleared() -> bool {
    critical_section::with(|cs| LIFECYCLE.borrow_ref(cs).shutdown_cleared())
}
/// RMT memory blocks for channel 1 (48 codes each on the S31).
const MEM_BLOCKS: u8 = 3;
/// RMT ticks per microsecond-tenth at the 10 MHz counter clock.
const T0H: u16 = 4;
const T0L: u16 = 8;
const T1H: u16 = 8;
const T1L: u16 = 4;

static GENERATION: AtomicU32 = AtomicU32::new(0);
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Acquire)
}
static READY: AtomicBool = AtomicBool::new(false);
static LUA_FRAME: critical_section::Mutex<core::cell::RefCell<Option<Frame>>> =
    critical_section::Mutex::new(core::cell::RefCell::new(None));

/// Successful startup black TX and latch, independent of board discovery.
/// Used only to bound early boot waiting; it does not acknowledge shutdown.
pub fn startup_cleared() -> bool {
    READY.load(Ordering::Acquire)
}
pub fn available() -> bool {
    READY.load(Ordering::Acquire) && ATTACHED.load(Ordering::Acquire) && !shutting_down()
}
pub fn lua_frame(pixels: [[u8; 3]; 6]) -> Result<(), i32> {
    if shutting_down() {
        return Err(lua_engine::esp_err::INVALID_STATE);
    }
    if !available() {
        return Err(lua_engine::esp_err::NOT_SUPPORTED);
    }
    if OWNER.load(Ordering::Acquire) != Owner::Lua as u8 {
        return Err(lua_engine::esp_err::INVALID_STATE);
    }
    let frame = pixels.map(|[r, g, b]| Rgb { r, g, b });
    critical_section::with(|cs| *LUA_FRAME.borrow_ref_mut(cs) = Some(frame));
    Ok(())
}
pub fn lua_release() {
    critical_section::with(|cs| *LUA_FRAME.borrow_ref_mut(cs) = None);
}

static BRAIN_FRAME: critical_section::Mutex<core::cell::RefCell<Option<(Frame, u64)>>> =
    critical_section::Mutex::new(core::cell::RefCell::new(None));
static REFLEX_UNTIL: critical_section::Mutex<core::cell::Cell<u64>> =
    critical_section::Mutex::new(core::cell::Cell::new(0));
/// Independent resident lease: never changes the foreground owner or frame.
pub fn brain_frame(pixels: Option<[[u8; 3]; 6]>, now: u64) {
    if shutting_down() {
        return;
    }
    critical_section::with(|cs| {
        *BRAIN_FRAME.borrow_ref_mut(cs) =
            pixels.map(|p| (p.map(|[r, g, b]| Rgb { r, g, b }), now.saturating_add(200)))
    });
}
pub fn native_reflex(now: u64, duration: u64) {
    if shutting_down() {
        return;
    }
    critical_section::with(|cs| {
        let end = REFLEX_UNTIL.borrow(cs);
        end.set(end.get().max(now.saturating_add(duration)));
    });
}
static NATIVE_EXPRESSION: AtomicU8 = AtomicU8::new(0);
static EXPRESSION: AtomicU8 = AtomicU8::new(0);
static AMBIENT: AtomicU8 = AtomicU8::new(u8::MAX);
static ATTACHED: AtomicBool = AtomicBool::new(false);
/// Settings switch: when false the ring shows black (RFC 0006).
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Key presses: low byte counts left presses, next byte right presses.
static SPARKS: AtomicU32 = AtomicU32::new(0);
/// Who drives the strip, see [`Owner`].
static OWNER: AtomicU8 = AtomicU8::new(Owner::Fox as u8);
/// Bumped to restart the Settings preview wipe.
static PREVIEW_RUNS: AtomicU32 = AtomicU32::new(0);

/// Who drives the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Owner {
    /// The fox's mood light (Home).
    Fox = 0,
    /// The Settings preview: a wipe into steady warm white.
    Preview = 1,
    /// Dark (other apps).
    Dark = 2,
    /// A bounded six-pixel frame submitted by the foreground Lua app.
    Lua = 3,
}

/// Hand the strip to `owner`. Entering the preview restarts its wipe.
pub fn set_owner(owner: Owner) {
    if shutting_down() {
        return;
    }
    let old = OWNER.swap(owner as u8, Ordering::Relaxed);
    if old != owner as u8 {
        lua_release();
    }
    if owner == Owner::Preview && old != owner as u8 {
        replay_preview();
    }
}

/// Restart the Settings preview wipe (the lights were just switched on).
pub fn replay_preview() {
    if shutting_down() {
        return;
    }
    PREVIEW_RUNS.fetch_add(1, Ordering::Relaxed);
}

/// The expression on screen. Called by the UI task each loop.
pub fn show_native(expression: Expression) {
    if shutting_down() {
        return;
    }
    let index = Expression::ALL
        .iter()
        .position(|e| *e == expression)
        .unwrap_or(0);
    NATIVE_EXPRESSION.store(index as u8, Ordering::Relaxed);
}
pub fn show(expression: Expression) {
    if shutting_down() {
        return;
    }
    let index = Expression::ALL
        .iter()
        .position(|e| *e == expression)
        .unwrap_or(0);
    EXPRESSION.store(index as u8, Ordering::Relaxed);
}

/// Relative room light, 0..=100. Called by the sensor task.
pub fn set_ambient(level: u8) {
    AMBIENT.store(level.min(100), Ordering::Relaxed);
}

/// A key on the Interaction board went down.
pub fn spark(side: Side) {
    if shutting_down() {
        return;
    }
    let shift = match side {
        Side::Left => 0,
        Side::Right => 8,
    };
    SPARKS.fetch_add(1 << shift, Ordering::Relaxed);
}

/// Turn the mood lights on or off (Settings). Off writes black frames, so
/// the ring goes dark at once; the engine keeps running underneath.
pub fn set_enabled(enabled: bool) {
    if shutting_down() {
        return;
    }
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether an Interaction board is the active one in the right slot.
pub fn set_attached(attached: bool) {
    if ATTACHED.swap(attached, Ordering::AcqRel) != attached {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        lua_release();
    }
}

/// Configure RMT channel 1 for WS2812. The caller attaches the data pin.
pub fn configure_channel(
    creator: ChannelCreator<'static, Blocking, 1>,
) -> Result<LightChannel, esp_hal::rmt::ConfigError> {
    let config = TxChannelConfig::default()
        .with_clk_divider(1)
        .with_memsize(MEM_BLOCKS)
        .with_idle_output(true)
        .with_idle_output_level(Level::Low);
    creator.configure_tx(&config)
}

#[embassy_executor::task]
pub async fn lights_task(mut channel: LightChannel) {
    info!("lights_task: WS2812 x{LED_COUNT} on GPIO38 (right slot), RMT ch1 10 MHz");
    let mut light = MoodLight::new();
    let mut native_light = MoodLight::new();
    let mut codes = [PulseCode::end_marker(); LED_COUNT * 24 + 1];
    let mut sent: Option<Frame> = None;
    let mut sent_at = 0u64;
    let mut seen_sparks = SPARKS.load(Ordering::Relaxed);
    let mut was_attached = false;
    let mut errors = 0u32;
    let mut seen_previews = PREVIEW_RUNS.load(Ordering::Relaxed);
    let mut preview_since = 0u64;

    loop {
        Timer::after(Duration::from_millis(FRAME_MS)).await;
        let now = Instant::now().as_millis();
        let (clear, allows_color) = critical_section::with(|cs| {
            let p = LIFECYCLE.borrow_ref(cs);
            (p.next_clear(), p.allows_color())
        });
        // No discovery, owner, setting, cache or colour source can suppress a
        // new clear. Once acknowledged, shutdown never transmits colour again.
        if clear.is_none() && !allows_color {
            continue;
        }

        let index = EXPRESSION.load(Ordering::Relaxed) as usize;
        light.set_expression(Expression::ALL[index.min(Expression::ALL.len() - 1)], now);
        let native_index = NATIVE_EXPRESSION.load(Ordering::Relaxed) as usize;
        native_light.set_expression(
            Expression::ALL[native_index.min(Expression::ALL.len() - 1)],
            now,
        );
        let ambient = AMBIENT.load(Ordering::Relaxed);
        if ambient <= 100 {
            light.set_ambient(ambient);
            native_light.set_ambient(ambient);
        }
        let sparks = SPARKS.load(Ordering::Relaxed);
        if sparks & 0xFF != seen_sparks & 0xFF {
            light.spark(Side::Left, now);
            native_light.spark(Side::Left, now);
        }
        if (sparks >> 8) & 0xFF != (seen_sparks >> 8) & 0xFF {
            light.spark(Side::Right, now);
            native_light.spark(Side::Right, now);
        }
        seen_sparks = sparks;

        let previews = PREVIEW_RUNS.load(Ordering::Relaxed);
        if previews != seen_previews {
            seen_previews = previews;
            preview_since = now;
        }
        // The fox's light keeps running underneath so it resumes smoothly.
        let fox = light.frame(now);
        let native = native_light.frame(now);
        let mut frame = match OWNER.load(Ordering::Relaxed) {
            0 => critical_section::with(|cs| {
                if now < REFLEX_UNTIL.borrow(cs).get()
                    || matches!(
                        Expression::ALL[native_index.min(Expression::ALL.len() - 1)],
                        Expression::Error | Expression::Panic
                    )
                {
                    return native;
                }
                BRAIN_FRAME
                    .borrow_ref(cs)
                    .as_ref()
                    .filter(|(_, until)| now < *until)
                    .map(|(frame, _)| *frame)
                    .unwrap_or(fox)
            }),
            1 => preview_frame(now.saturating_sub(preview_since)),
            3 => critical_section::with(|cs| {
                (*LUA_FRAME.borrow_ref(cs)).unwrap_or([Rgb::OFF; LED_COUNT])
            }),
            _ => [Rgb::OFF; LED_COUNT],
        };
        if !allows_color || !ENABLED.load(Ordering::Relaxed) || crate::ota_runtime::active() {
            frame.iter_mut().for_each(|led| *led = Default::default());
        }
        let attached = ATTACHED.load(Ordering::Relaxed);
        if attached != was_attached {
            info!(
                "lights: {}",
                if attached {
                    "active right board"
                } else {
                    "no active right board"
                }
            );
            was_attached = attached;
            sent = None;
        }
        if !attached && clear.is_none() {
            continue;
        }
        if clear.is_none() && sent == Some(frame) && now.saturating_sub(sent_at) < REFRESH_MS {
            continue;
        }

        encode(&frame, &mut codes);
        // poll() refills the 144-code memory before its marker and reports
        // terminal End/Error. Only then does HAL guarantee wait() is immediate.
        let outcome = match channel.transmit(&codes) {
            Ok(mut tx) => {
                let started = Instant::now();
                while !tx.poll() {
                    if started.elapsed().as_micros() >= TX_TIMEOUT_US {
                        drop(tx); // HAL guard aborts TX; channel is consumed.
                        READY.store(false, Ordering::Release);
                        warn!("lights: TX timeout; output unavailable, no clear receipt");
                        // The public HAL has no abort-and-return-channel API.
                        // Stay yielded with the terminal gate unacknowledged.
                        loop {
                            Timer::after_millis(FRAME_MS).await;
                        }
                    }
                }
                tx.wait()
            }
            Err(error) => Err(error),
        };
        match outcome {
            Ok(returned) => {
                channel = returned;
                sent = Some(frame);
                sent_at = now;
                if let Some(attempt) = clear {
                    Timer::after_micros(LATCH_US).await;
                    critical_section::with(|cs| LIFECYCLE.borrow_ref_mut(cs).cleared(attempt));
                    if attempt == shutdown::Clear::Startup {
                        READY.store(true, Ordering::Release);
                    }
                    info!("lights: {attempt:?} black TX complete; latch >=280us");
                }
            }
            Err((error, returned)) => {
                channel = returned;
                errors = errors.saturating_add(1);
                if errors.is_power_of_two() {
                    warn!("lights: TX failed ({errors}x), no clear receipt: {error:?}");
                }
            }
        }
    }
}

/// Encode `frame` as WS2812 bits (GRB, MSB first) followed by an end marker.
fn encode(frame: &Frame, codes: &mut [PulseCode; LED_COUNT * 24 + 1]) {
    let mut i = 0;
    for Rgb { r, g, b } in frame {
        for byte in [*g, *r, *b] {
            for bit in (0..8).rev() {
                let (high, low) = if byte & (1 << bit) != 0 {
                    (T1H, T1L)
                } else {
                    (T0H, T0L)
                };
                codes[i] = PulseCode::new(Level::High, high, Level::Low, low);
                i += 1;
            }
        }
    }
    codes[i] = PulseCode::end_marker();
}
