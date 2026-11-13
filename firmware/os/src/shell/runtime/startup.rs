//! The first main-application screen. It shares the normal shadow/plane lease
//! and releases all decoder storage when the selected character takes over.
use crate::formats::boot_animation::{self as format, Animation, Window, FRAME_BYTES, WIDTH};
use crate::platform::runtime::buffers::ExternalBytes;
use crate::services::display::{
    lvgl_port::DisplayPort,
    plane::{Frame, PlaneJob, PlaneUi},
};

const BUNDLE: &[u8] = include_bytes!("../../assets/boot.ban");
const ROWS: usize = 32;

pub(super) struct Startup {
    animation: Animation<'static>,
    buffers: ExternalBytes,
    band: ExternalBytes,
    deadline_ms: u64,
    started: bool,
}
impl Startup {
    pub fn new() -> Result<Self, &'static str> {
        Ok(Self {
            animation: Animation::parse(BUNDLE).map_err(|_| "invalid startup animation")?,
            buffers: ExternalBytes::try_zeroed(2 * FRAME_BYTES)?,
            band: ExternalBytes::try_zeroed(WIDTH * ROWS * 2)?,
            deadline_ms: 0,
            started: false,
        })
    }
    pub fn render(
        &mut self,
        now_ms: u64,
        port: &mut DisplayPort,
        plane: &mut PlaneUi,
    ) -> Result<(), &'static str> {
        if port.is_busy() || now_ms < self.deadline_ms || self.animation.finished() {
            return Ok(());
        }
        let (current, scratch) = self.buffers.split_at_mut(FRAME_BYTES);
        let mut damage: Option<Window> = None;
        let first = !self.started;
        if first {
            self.deadline_ms = now_ms;
        }
        // If a scheduling pause missed a deadline, apply every delta in order
        // but present only the current picture. Playback never accumulates lag.
        loop {
            let frame = self
                .animation
                .advance(current, scratch)
                .map_err(|_| "startup frame decode failed")?;
            if let Some(w) = frame.window {
                damage = Some(match damage {
                    None => w,
                    Some(old) => Window {
                        x0: old.x0.min(w.x0),
                        y0: old.y0.min(w.y0),
                        x1: old.x1.max(w.x1),
                        y1: old.y1.max(w.y1),
                    },
                });
            }
            self.deadline_ms += u64::from(frame.duration_ms);
            if self.deadline_ms > now_ms || self.animation.finished() {
                break;
            }
        }
        if first {
            port.discard_damage();
            damage = Some(Window {
                x0: 0,
                y0: 0,
                x1: 480,
                y1: 480,
            });
            self.started = true;
            log::info!(
                "startup: animation submitted app_ms={} ({} frames, {} bytes)",
                now_ms,
                self.animation.frames(),
                BUNDLE.len()
            );
        }
        if let Some(w) = damage {
            let stride = w.width();
            for y in (w.y0 as usize..w.y1 as usize).step_by(ROWS) {
                let rows = (w.y1 as usize - y).min(ROWS);
                for row in 0..rows {
                    let source = ((y + row) * WIDTH + w.x0 as usize) / 2;
                    let target = row * stride * 2;
                    format::expand_rgb565(
                        &current[source..source + stride / 2],
                        &mut self.band[target..target + stride * 2],
                    )
                    .map_err(|_| "startup pixel expansion failed")?;
                }
                // SAFETY: this initialized band contains rows * stride RGB565
                // pixels. is_busy was false; only this UI owner writes shadow.
                unsafe {
                    port.write_rect(
                        (w.x0, y as u16, w.x1, (y + rows) as u16),
                        self.band.as_ptr(),
                        stride * 2,
                    );
                }
            }
            if let Some((screen, damage)) = port.take_frame() {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Screen(screen),
                    size: (480, 480),
                    at: Some((0, 0)),
                    damage: Some(damage),
                    clear_outside: false,
                    urgent: first,
                });
            }
        }
        Ok(())
    }
}
