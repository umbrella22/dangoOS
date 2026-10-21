//! Interaction board WS2812 mood light.
//!
//! [`mosaico_s31::behavior::mood_light::MoodLight`] decides the colours; this
//! task sends them to the six LEDs every [`FRAME_MS`].
//!
//! - Data pin: right slot `GPIO38` (BSP `ws2812_io`, mirrored from the left
//!   slot's `GPIO15`). The left slot is not wired: `GPIO15` is also the
//!   camera's D1, and an RMT channel cannot move between pins at runtime.
//! - Timing: RMT channel 1 at a 10 MHz tick (0.1 us), WS2812 800 kHz
//!   (0: 0.4/0.8 us, 1: 0.8/0.4 us), GRB order, MSB first. The frame gap is
//!   far longer than the 280 us latch time.
//! - Channel 1 takes three RMT memory blocks (144 codes) so one frame fits
//!   with a single refill for the end marker.
//! - Other tasks only touch atomics: [`show`], [`set_ambient`], [`spark`] and
//!   [`set_attached`]. Nothing here blocks them.
//! - Frames are only sent while an Interaction board is in the right slot,
//!   and only when they change (plus once a second as a refresh).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    gpio::Level,
    rmt::{Channel, ChannelCreator, PulseCode, Tx, TxChannelConfig, TxChannelCreator},
    Blocking,
};
use log::{info, warn};
use mosaico_s31::{
    behavior::mood_light::{Frame, MoodLight, Rgb, Side, LED_COUNT},
    Expression,
};

/// RMT TX channel 1, attached to `GPIO38` in `main`.
pub type LightChannel = Channel<'static, Blocking, Tx>;

/// Frame period (50 Hz).
pub const FRAME_MS: u64 = 20;
/// Resend an unchanged frame this often, in case a bit was lost.
const REFRESH_MS: u64 = 1000;
/// RMT memory blocks for channel 1 (48 codes each on the S31).
const MEM_BLOCKS: u8 = 3;
/// RMT ticks per microsecond-tenth at the 10 MHz counter clock.
const T0H: u16 = 4;
const T0L: u16 = 8;
const T1H: u16 = 8;
const T1L: u16 = 4;

static EXPRESSION: AtomicU8 = AtomicU8::new(0);
static AMBIENT: AtomicU8 = AtomicU8::new(u8::MAX);
static ATTACHED: AtomicBool = AtomicBool::new(false);
/// Settings switch: when false the ring shows black (RFC 0006).
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Key presses: low byte counts left presses, next byte right presses.
static SPARKS: AtomicU32 = AtomicU32::new(0);

/// The expression on screen. Called by the UI task each loop.
pub fn show(expression: Expression) {
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
    let shift = match side {
        Side::Left => 0,
        Side::Right => 8,
    };
    SPARKS.fetch_add(1 << shift, Ordering::Relaxed);
}

/// Turn the mood lights on or off (Settings). Off writes black frames, so
/// the ring goes dark at once; the engine keeps running underneath.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Whether an Interaction board is the active one in the right slot.
pub fn set_attached(attached: bool) {
    ATTACHED.store(attached, Ordering::Relaxed);
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
    let mut codes = [PulseCode::end_marker(); LED_COUNT * 24 + 1];
    let mut sent: Option<Frame> = None;
    let mut sent_at = 0u64;
    let mut seen_sparks = SPARKS.load(Ordering::Relaxed);
    let mut was_attached = false;
    let mut errors = 0u32;

    loop {
        Timer::after(Duration::from_millis(FRAME_MS)).await;
        let now = Instant::now().as_millis();

        let index = EXPRESSION.load(Ordering::Relaxed) as usize;
        light.set_expression(Expression::ALL[index.min(Expression::ALL.len() - 1)], now);
        let ambient = AMBIENT.load(Ordering::Relaxed);
        if ambient <= 100 {
            light.set_ambient(ambient);
        }
        let sparks = SPARKS.load(Ordering::Relaxed);
        if sparks & 0xFF != seen_sparks & 0xFF {
            light.spark(Side::Left, now);
        }
        if (sparks >> 8) & 0xFF != (seen_sparks >> 8) & 0xFF {
            light.spark(Side::Right, now);
        }
        seen_sparks = sparks;

        let mut frame = light.frame(now);
        if !ENABLED.load(Ordering::Relaxed) {
            frame.iter_mut().for_each(|led| *led = Default::default());
        }
        let attached = ATTACHED.load(Ordering::Relaxed);
        if attached != was_attached {
            info!("lights: {}", if attached { "on" } else { "off (no board)" });
            was_attached = attached;
            sent = None;
        }
        if !attached {
            continue;
        }
        if sent == Some(frame) && now.saturating_sub(sent_at) < REFRESH_MS {
            continue;
        }

        encode(&frame, &mut codes);
        channel = match channel.transmit(&codes) {
            Ok(tx) => match tx.wait() {
                Ok(channel) => {
                    sent = Some(frame);
                    sent_at = now;
                    channel
                }
                Err((error, channel)) => {
                    errors += 1;
                    if errors.is_power_of_two() {
                        warn!("lights: transmit failed ({errors}x): {error:?}");
                    }
                    channel
                }
            },
            Err((error, channel)) => {
                errors += 1;
                if errors.is_power_of_two() {
                    warn!("lights: transmit rejected ({errors}x): {error:?}");
                }
                channel
            }
        };
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
