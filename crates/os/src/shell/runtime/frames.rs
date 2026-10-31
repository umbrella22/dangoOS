//! Frame publication and slide progression on the existing display leases.
use super::*;
/// A page slide between the launcher and an app (`hub::transition`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slide {
    Idle,
    /// Waiting for the plane to release the shadow so the old page can be
    /// copied into the snapshot.
    Pending(Direction),
    /// Uploading slide frames.
    Running {
        direction: Direction,
        start_ms: u64,
    },
    /// One last full upload of the new page.
    Finishing,
}

pub(crate) struct Frames {
    pub slide: Slide,
    pub clear_on_fox: bool,
    pub last_camera_frame: u32,
    pub management_frame_submitted: bool,
    pub saving_frame_submitted: u32,
    pub skin_loading_rendered: bool,
    pub skin_loading_frame_submitted: bool,
}
impl Frames {
    pub const fn new() -> Self {
        Self {
            slide: Slide::Idle,
            clear_on_fox: false,
            last_camera_frame: 0,
            management_frame_submitted: false,
            saving_frame_submitted: 0,
            skin_loading_rendered: false,
            skin_loading_frame_submitted: false,
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render(
        &mut self,
        ui: &mut Ui<AppMsg>,
        app: &mut MosaicoApp,
        control: &FoxControl,
        port: &mut DisplayPort,
        plane: &mut PlaneUi,
        elapsed_ms: u32,
        now_ms: u64,
        plane_size: (u16, u16),
        pending_touch: &mut bool,
        pending_management: bool,
        preferences: crate::services::settings::device::Preferences,
    ) {
        // ---- render into a free slot, then hand the frame to the plane ----
        let rendered_before_tick = control.rendered_frames();
        if let Err(error) = ui.tick(elapsed_ms) {
            warn!("ui_task: Ui::tick rejected: {error}");
        }
        if app.page() == Page::Home {
            if let Some(canvas) = control.take_frame() {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: buttons::shutdown_requested()
                        && control.shutdown_sleep_frame_since(rendered_before_tick),
                    canvas: Frame::Pool(canvas),
                    size: plane_size,
                    at: None,
                    damage: None,
                    clear_outside: core::mem::take(&mut self.clear_on_fox),
                    urgent: core::mem::take(pending_touch),
                });
            }
        } else if self.slide != Slide::Idle {
            self.slide = step_slide(self.slide, port, plane, now_ms);
        } else if is_lua_page(app.page()) && lua::screen_open() {
            // The script owns the screen: copy its presented frame into the
            // shadow and upload the damage; LVGL stays paused.
            if !port.is_busy() {
                if let Some(blits) = lua::take_frame() {
                    for b in &blits {
                        // SAFETY: the script's framebuffer stays untouched
                        // until `frame_done` below (it waits for it).
                        unsafe {
                            port.write_rect((b.x0, b.y0, b.x1, b.y1), b.pixels, b.stride);
                        }
                    }
                    lua::frame_done();
                }
                if let Some((screen, damage)) = port.take_frame() {
                    plane.submit(PlaneJob {
                        shutdown_low_battery: false,
                        shutdown_sleep: false,
                        canvas: Frame::Screen(screen),
                        size: (WIDTH as u16, HEIGHT as u16),
                        at: Some((0, 0)),
                        damage: Some(damage),
                        clear_outside: false,
                        urgent: false,
                    });
                }
            }
        } else if !port.is_busy() {
            let skin_feedback_on_frame = self.skin_loading_rendered;
            ui.timer_handler();

            // Callbacks fired by the timer handler only queue messages.
            match ui.drain(app, DRAIN_MAX) {
                Ok(_) => {
                    self.skin_loading_rendered = app.page() == Page::App(AppId::Settings)
                        && app.settings_page()
                            == crate::shell::pages::settings::model::Page::Character
                        && app.settings_session().loading_skin.is_some();
                }
                Err(error) => warn!("ui_task: Ui::drain rejected: {error}"),
            }

            apps::set_ui_management_pending(app.management_busy());
            if app.page() == Page::App(AppId::Camera) {
                if let Some(frame) = crate::services::camera::task::frame() {
                    if frame.sequence != self.last_camera_frame {
                        // The Arc retains the completed CPU buffer while the
                        // shadow receives it; camera DMA uses a different buffer.
                        unsafe {
                            port.write_rect(
                                (60, 96, 420, 366),
                                frame.pixels.as_ptr().cast(),
                                360 * 2,
                            );
                        }
                        self.last_camera_frame = frame.sequence;
                    }
                }
            }
            if let Some((screen, damage)) = port.take_frame() {
                if skin_feedback_on_frame {
                    self.skin_loading_frame_submitted = true;
                }
                // This is the ordinary LVGL busy-state frame, never a Lua
                // or transition frame. Its lease must finish before deletion.
                if pending_management && app.page() == Page::Launcher {
                    self.management_frame_submitted = true;
                }
                if app.page() == Page::App(AppId::Settings)
                    && !matches!(
                        app.settings_page(),
                        crate::shell::pages::settings::model::Page::Password
                            | crate::shell::pages::settings::model::Page::Proxy
                    )
                    && app.settings_session().info.saving
                {
                    self.saving_frame_submitted = preferences.saving_revision;
                }
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Screen(screen),
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: Some(damage),
                    clear_outside: false,
                    urgent: false,
                });
            }
        }
    }
}
/// Advance a page slide by one UI loop. LVGL is paused throughout: the new
/// page is rendered once up front and stays in the shadow.
fn step_slide(slide: Slide, port: &mut DisplayPort, plane: &mut PlaneUi, now_ms: u64) -> Slide {
    match slide {
        Slide::Idle => Slide::Idle,
        Slide::Pending(direction) => {
            if !port.snapshot() {
                return slide;
            }
            port.render_now();
            Slide::Running {
                direction,
                start_ms: now_ms,
            }
        }
        Slide::Running {
            direction,
            start_ms,
        } => {
            let elapsed = now_ms.saturating_sub(start_ms);
            if elapsed >= transition::SLIDE_MS {
                return step_slide(Slide::Finishing, port, plane, now_ms);
            }
            if let Some((new, old)) = port.take_slide() {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Slide {
                        new,
                        old,
                        direction,
                        offset: transition::offset(elapsed, WIDTH as u16),
                    },
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: None,
                    clear_outside: false,
                    urgent: false,
                });
            }
            slide
        }
        Slide::Finishing => match port.take_full_frame() {
            Some((screen, damage)) => {
                plane.submit(PlaneJob {
                    shutdown_low_battery: false,
                    shutdown_sleep: false,
                    canvas: Frame::Screen(screen),
                    size: (WIDTH as u16, HEIGHT as u16),
                    at: Some((0, 0)),
                    damage: Some(damage),
                    clear_outside: false,
                    urgent: false,
                });
                port.reset_pointer();
                Slide::Idle
            }
            None => slide,
        },
    }
}
