//! One indexed frame and one reusable DMA band; no per-frame heap allocation.
use alloc::vec;
use board_s31::{display::Display, geometry::DirtyRect};
use core::sync::atomic::Ordering;
#[cfg(feature = "boot-animation")]
use embassy_time::Instant;
use embassy_time::Timer;
use esp_hal::Async;

const STATIC: &[u8] = include_bytes!("../assets/static.rcs");
const WAITING: &[u8] = include_bytes!("../assets/waiting.rct");
const FONT: &[u8] = include_bytes!("../assets/ascii-12x20.bin");
#[cfg(feature = "boot-animation")]
const ANIMATION: &[u8] = include_bytes!("../assets/animation.ran");
const PIXELS: usize = 480 * 480;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Footer {
    connected: bool,
    cancelled: bool,
    active: u8,
    position: u32,
    writing: bool,
    install_phase: u8,
    install_position: u32,
}
impl Footer {
    fn current() -> Self {
        Self {
            connected: super::CONNECTED.load(Ordering::Acquire),
            cancelled: super::CANCELLED.load(Ordering::Acquire),
            active: super::ACTIVE.load(Ordering::Acquire),
            position: super::POSITION.load(Ordering::Acquire),
            writing: super::WRITE_MODE.load(Ordering::Acquire),
            install_phase: super::INSTALL_PHASE.load(Ordering::Acquire),
            install_position: super::INSTALL_POSITION.load(Ordering::Acquire),
        }
    }
    fn status(self) -> &'static str {
        if self.cancelled {
            return "CANCELLED";
        }
        if self.writing {
            return match self.install_phase {
                1 => "CHECKING PACKAGE",
                3 => "VERIFYING MAIN",
                _ => "INSTALLING MAIN",
            };
        }
        if self.install_phase == 5 {
            return "INSTALL FAILED / USB READY";
        }
        if !self.connected {
            return "WAITING USB";
        }
        match self.active {
            2 => "READING NOR",
            3 => "READING NAND",
            4 => "MOUNT READ ONLY",
            5 | 6 => "EXTRACTING FILE",
            7 => "READING VOLUME",
            0x10 | 0x11 => "INSTALLING MAIN",
            0x12 => "VERIFYING MAIN",
            _ => "USB READY",
        }
    }
}
fn text(band: &mut [u16], band_y: usize, x: usize, y: usize, s: &str, color: u16) {
    for (column, c) in s.bytes().enumerate() {
        if !(32..127).contains(&c) {
            continue;
        }
        for row in 0..20 {
            if y + row < band_y || y + row >= band_y + 32 {
                continue;
            }
            let at = (c as usize - 32) * 40 + row * 2;
            let bits = u16::from_le_bytes([FONT[at], FONT[at + 1]]);
            for gx in 0..12 {
                let px = x + column * 12 + gx;
                if px < 480 && bits & (1 << (15 - gx)) != 0 {
                    band[(y + row - band_y) * 480 + px] = color;
                }
            }
        }
    }
}
fn static_frame(out: &mut [u8]) -> bool {
    STATIC.len() >= 40
        && &STATIC[..8] == b"RCS1\xe0\x01\xe0\x01"
        && miniz_oxide::inflate::decompress_slice_iter_to_slice(
            out,
            core::iter::once(&STATIC[40..]),
            true,
            false,
        )
        .ok()
            == Some(PIXELS / 2)
}
fn waiting(band: &mut [u16], band_y: usize) {
    if WAITING.len() != 2050 || &WAITING[..10] != b"RCT1\xe0\x01\x22\x00\x86\x01" {
        text(band, band_y, 174, 396, "WAITING USB", 0xffff);
        return;
    }
    for y in 390..424 {
        if y < band_y || y >= band_y + 32 {
            continue;
        }
        for x in 0..480 {
            let bit = (y - 390) * 480 + x;
            if WAITING[10 + bit / 8] & (0x80 >> (bit % 8)) != 0 {
                band[(y - band_y) * 480 + x] = 0xffff;
            }
        }
    }
}
async fn render(
    panel: &mut Display<Async>,
    band: &mut [u16],
    frame: Option<&[u8]>,
    palette: &[u16; 16],
    footer: Footer,
    footer_only: bool,
) -> Result<(), board_s31::display::Error> {
    for y in (0..480).step_by(32) {
        if footer_only && y + 32 <= 380 {
            continue;
        }
        let rows = (480 - y).min(32);
        band.fill(0);
        if let Some(frame) = frame {
            for (i, pixel) in band[..rows * 480].iter_mut().enumerate() {
                let at = y * 480 + i;
                if at / 480 >= 380 {
                    break;
                }
                let index = if at % 2 == 0 {
                    frame[at / 2] >> 4
                } else {
                    frame[at / 2] & 15
                };
                *pixel = palette[index as usize];
            }
        } else {
            text(band, y, 192, 216, "RECOVERY", 0xffff);
        }
        if !footer.connected && !footer.cancelled && !footer.writing && footer.install_phase != 5 {
            waiting(band, y);
        } else {
            let status = footer.status();
            text(band, y, (480 - status.len() * 12) / 2, 396, status, 0xffff);
        }
        let label = if footer.writing {
            "INSTALL MODE / TAP TO CANCEL"
        } else {
            "READ ONLY / TAP TO CANCEL"
        };
        text(band, y, (480 - label.len() * 12) / 2, 444, label, 0xa514);
        let dst = DirtyRect {
            x0: 0,
            y0: y as u16,
            x1: 479,
            y1: (y + rows - 1) as u16,
        };
        let src = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 479,
            y1: (rows - 1) as u16,
        };
        panel
            .write_canvas_async(dst, src, (480, rows as u16), &band[..rows * 480])
            .await?;
    }
    Ok(())
}

// The palette's zero is black. Updating the union of the current and previous
// visible bounds also clears objects that moved or disappeared, without a
// second framebuffer. Bounds include both pixels of each indexed byte.
#[cfg(feature = "boot-animation")]
fn bounds(frame: &[u8]) -> Option<DirtyRect> {
    crate::animation::visible_bounds(frame).map(|r| DirtyRect {
        x0: r.x0,
        y0: r.y0,
        x1: r.x1,
        y1: r.y1,
    })
}
#[cfg(feature = "boot-animation")]
async fn render_region(
    panel: &mut Display<Async>,
    band: &mut [u16],
    frame: &[u8],
    palette: &[u16; 16],
    region: DirtyRect,
) -> Result<(), board_s31::display::Error> {
    let width = (region.x1 - region.x0 + 1) as usize;
    for y in (region.y0 as usize..=region.y1 as usize).step_by(32) {
        let rows = (region.y1 as usize + 1 - y).min(32);
        for (row, out) in band[..rows * width].chunks_exact_mut(width).enumerate() {
            let at = (y + row) * 240 + region.x0 as usize / 2;
            for (pair, &indices) in out.chunks_exact_mut(2).zip(&frame[at..at + width / 2]) {
                pair[0] = palette[(indices >> 4) as usize];
                pair[1] = palette[(indices & 15) as usize];
            }
        }
        panel
            .write_canvas_async(
                DirtyRect {
                    x0: region.x0,
                    y0: y as u16,
                    x1: region.x1,
                    y1: (y + rows - 1) as u16,
                },
                DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: width as u16 - 1,
                    y1: rows as u16 - 1,
                },
                (width as u16, rows as u16),
                &band[..rows * width],
            )
            .await?;
    }
    Ok(())
}

#[cfg(feature = "boot-animation")]
async fn intro(
    panel: &mut Display<Async>,
    band: &mut [u16],
    frame: &mut [u8],
    palette: &mut [u16; 16],
) -> bool {
    let Ok(animation) = crate::animation::Animation::parse(ANIMATION) else {
        return false;
    };
    *palette = core::array::from_fn(|i| animation.color(i as u8));
    let start = Instant::now();
    let mut due = start;
    let mut previous_region: Option<DirtyRect> = None;
    let mut previous_footer = None;
    for index in 0..animation.frames() {
        let state = Footer::current();
        // USB has already started. Media work and physical interaction can
        // interrupt the intro; they do not wait for its last animation frame.
        if state.cancelled
            || state.writing
            || !matches!(state.active, 0 | 1 | 8)
            || super::TOUCH_EVENTS.load(Ordering::Acquire) != 0
        {
            break;
        }
        let decode_start = Instant::now();
        let Ok(duration) = animation.decode(index, frame) else {
            return false;
        };
        super::INTRO_DECODE_US
            .fetch_add(decode_start.elapsed().as_micros() as u32, Ordering::Relaxed);
        let draw_start = Instant::now();
        let current_region = bounds(frame);
        if index == 0 || palette[0] != 0 {
            if render(panel, band, Some(frame), palette, state, false)
                .await
                .is_err()
            {
                return false;
            }
        } else {
            let region = match (previous_region, current_region) {
                (Some(a), Some(b)) => Some(DirtyRect {
                    x0: a.x0.min(b.x0),
                    y0: a.y0.min(b.y0),
                    x1: a.x1.max(b.x1),
                    y1: a.y1.max(b.y1),
                }),
                (a, b) => a.or(b),
            };
            if let Some(region) = region {
                if render_region(panel, band, frame, palette, region)
                    .await
                    .is_err()
                {
                    return false;
                }
            }
            if previous_footer != Some(state)
                && render(panel, band, Some(frame), palette, state, true)
                    .await
                    .is_err()
            {
                return false;
            }
        }
        previous_region = current_region;
        previous_footer = Some(state);
        super::INTRO_DRAW_US.fetch_add(draw_start.elapsed().as_micros() as u32, Ordering::Relaxed);
        if index == 0 {
            if panel.set_brightness_async(35).await.is_err() {
                return false;
            }
            super::DISPLAY.store(1, Ordering::Release);
        }
        super::INTRO_FRAMES.store(index as u32 + 1, Ordering::Release);
        due += embassy_time::Duration::from_millis(duration as u64);
        Timer::at(due).await;
    }
    super::INTRO_MS.store(start.elapsed().as_millis() as u32, Ordering::Release);
    // A cancelled intro still settles on a complete, interactive final frame.
    animation.decode(animation.frames() - 1, frame).is_ok()
}

pub async fn run(mut panel: Display<Async>) {
    let mut frame = vec![0; PIXELS / 2];
    let mut band = vec![0u16; 480 * 32];
    let static_palette =
        || core::array::from_fn(|i| u16::from_le_bytes([STATIC[8 + i * 2], STATIC[9 + i * 2]]));
    #[cfg(feature = "boot-animation")]
    let (valid, palette) = {
        let mut palette = static_palette();
        if intro(&mut panel, &mut band, &mut frame, &mut palette).await {
            (true, palette)
        } else {
            (static_frame(&mut frame), static_palette())
        }
    };
    #[cfg(not(feature = "boot-animation"))]
    let (valid, palette) = (static_frame(&mut frame), static_palette());
    let mut first = true;
    let mut previous = None;
    loop {
        let state = Footer::current();
        if first || previous != Some(state) {
            if render(
                &mut panel,
                &mut band,
                valid.then_some(frame.as_slice()),
                &palette,
                state,
                !first,
            )
            .await
            .is_err()
            {
                super::DISPLAY.store(2, Ordering::Release);
                return;
            }
            if first {
                if panel.set_brightness_async(35).await.is_err() {
                    super::DISPLAY.store(2, Ordering::Release);
                    return;
                }
                super::DISPLAY.store(1, Ordering::Release);
            }
            first = false;
            previous = Some(state);
        }
        Timer::after_millis(100).await;
    }
}
