//! `FoxWidget`: the fox-dango character as an LVGL custom widget.
//!
//! The widget renders a 352x336 character and a small status band into one
//! two-slot [`CanvasPool`] at a bounded cadence. Application code drives the
//! mounted instance through a shared [`FoxControl`]: `rs_lvgl_ui::custom` keeps the
//! mounted widget when a later `App::view` builds a new descriptor, so the
//! handle is how the expression and pose change without a remount.
//!
//! # Lifecycle
//!
//! * `create` takes the control's canvas pool (allocated once per control and
//!   leaked, so its buffers are `'static`), creates a native canvas and
//!   centres it.
//! * `apply_props` accepts the unit properties without consuming animation time;
//!   live state continues to come from the shared control.
//! * `tick` advances the [`FoxAnimator`] on every `Ui::tick` and renders on
//!   a 50 ms cadence, or on the next tick after an urgent change, into a pool
//!   [`WriteLease`](crate::services::display::canvas_pool::WriteLease) and publishes it.
//!   - LVGL path (host previews and tests): the widget takes the published
//!     frame, points the native canvas at it and invalidates it.
//!   - Plane path (device): the UI task takes the frame with
//!     [`FoxControl::take_frame`] and hands it to the panel upload task. The
//!     native canvas is hidden and never drawn.
//! * `delete` only counts the deletion: the pool outlives every widget, so no
//!   LVGL object can ever point at freed pixels.

use alloc::rc::Rc;
use core::cell::Cell;
use core::ptr::NonNull;

use rs_lvgl::sys as lvgl_sys;
use rs_lvgl_ui::{CustomObject, CustomWidget, NativeObject, Obj, UiError};

use crate::companion::behavior::Expression;
use crate::companion::status;
use {
    crate::companion::render as fox, crate::companion::render::entrance,
    crate::companion::render::FoxAnimator, crate::companion::render::FoxInput,
    crate::companion::render::FoxPose, crate::companion::render::FoxRenderer,
};
use {
    crate::services::display::canvas_pool::CanvasPool,
    crate::services::display::canvas_pool::ReadLease,
};

/// Canvas width in pixels.
pub const CANVAS_WIDTH: usize = fox::W;
/// Canvas height in pixels.
pub const CANVAS_HEIGHT: usize = fox::H;
/// Each slot owns both images until LVGL or the panel finishes reading them.
pub const FRAME_PIXELS: usize = fox::PIXELS + status::PIXELS;

/// Minimum interval between rendered frames: 20 fps.
///
/// The animator still advances on every `Ui::tick`; this only bounds the
/// renderer and the panel upload. Urgent changes (a new expression, the rise
/// of a tap pulse) render on the next tick, so the cadence never delays a
/// touch response.
pub const FRAME_INTERVAL_MS: u32 = 50;

/// How far the pose may scale the fox above its normal size. The warp keeps
/// the bottom edge fixed, so larger values push the ears out of the canvas.
const MAX_POSE_SCALE: f32 = 1.10;

/// Per-frame pose on top of the expression: tap squash and gaze.
///
/// The expression and the pose are separate on purpose: the entrance animates
/// the pose while the face stays Idle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoxVisual {
    pub scale_x: f32,
    pub scale_y: f32,
    /// Horizontal gaze toward the finger, `-10..=10`. Idle eyes follow it.
    pub look_x: f32,
    /// Decaying tap pulse, `0..=1`.
    pub pulse: f32,
    /// Squash under a petting hand, `0..=1`.
    pub pet: f32,
    /// Lean with a stroke, canvas pixels.
    pub lean_x: f32,
}

impl FoxVisual {
    /// Unscaled, looking ahead, no pulse.
    pub const IDLE: Self = Self {
        scale_x: 1.0,
        scale_y: 1.0,
        look_x: 0.0,
        pulse: 0.0,
        pet: 0.0,
        lean_x: 0.0,
    };

    /// Clamp into the range the canvas can show.
    fn sanitized(self) -> Self {
        Self {
            scale_x: finite_or(self.scale_x, 1.0).clamp(0.4, MAX_POSE_SCALE),
            scale_y: finite_or(self.scale_y, 1.0).clamp(0.4, MAX_POSE_SCALE),
            look_x: finite_or(self.look_x, 0.0).clamp(-10.0, 10.0),
            pulse: finite_or(self.pulse, 0.0).clamp(0.0, 1.0),
            pet: finite_or(self.pet, 0.0).clamp(0.0, 1.0),
            lean_x: finite_or(self.lean_x, 0.0).clamp(-8.0, 8.0),
        }
    }
}

impl Default for FoxVisual {
    fn default() -> Self {
        Self::IDLE
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// Application-facing handle to the mounted [`FoxWidget`].
///
/// The application and the mounted widget each hold an `Rc` clone. The UI
/// task is single-threaded, so `Cell` is enough.
pub struct FoxControl {
    expression: Cell<Expression>,
    play_pose: Cell<Option<FoxPose>>,
    skin: core::cell::RefCell<Rc<fox::skin::Skin>>,
    loading: Cell<bool>,
    radio_status: Cell<Option<crate::services::radio::Snapshot>>,
    visual: Cell<FoxVisual>,
    /// Render on the next tick instead of waiting for the 50 ms cadence.
    urgent: Cell<bool>,
    #[cfg(test)]
    creates: Cell<u32>,
    #[cfg(test)]
    deletes: Cell<u32>,
    rendered: Cell<u64>,
    rendered_pose: Cell<Option<FoxPose>>,
    entrance_active: Cell<bool>,
    entrance_elapsed_ms: Cell<u64>,
    entrance_finished: Cell<bool>,
    /// When true the LVGL canvas is hidden and the UI task uploads frames
    /// through the panel plane instead.
    plane_mode: Cell<bool>,
    /// Canvas pool shared by every widget mounted on this control. Allocated
    /// on the first mount and never freed.
    pool: Cell<Option<&'static CanvasPool>>,
}

impl FoxControl {
    /// Create a control handle showing `expression`.
    pub fn new(expression: Expression) -> Rc<Self> {
        Rc::new(Self {
            expression: Cell::new(expression),
            play_pose: Cell::new(None),
            skin: core::cell::RefCell::new(fox::skin::Skin::builtin()),
            loading: Cell::new(false),
            radio_status: Cell::new(None),
            visual: Cell::new(FoxVisual::IDLE),
            urgent: Cell::new(true),
            #[cfg(test)]
            creates: Cell::new(0),
            #[cfg(test)]
            deletes: Cell::new(0),
            rendered: Cell::new(0),
            rendered_pose: Cell::new(None),
            entrance_active: Cell::new(false),
            entrance_elapsed_ms: Cell::new(0),
            entrance_finished: Cell::new(false),
            plane_mode: Cell::new(false),
            pool: Cell::new(None),
        })
    }

    /// Update the status band sent with the character to LVGL or the plane.
    /// A changed measurement renders on the next UI tick.
    pub fn set_radio_status(&self, status: crate::services::radio::Snapshot) {
        if self.radio_status.replace(Some(status)) != Some(status) {
            self.mark_urgent();
        }
    }

    /// Upload frames through the panel plane instead of LVGL.
    ///
    /// Must be set before the widget is mounted. Frames still render through
    /// [`FoxWidget`], but the native canvas is hidden; the UI task collects
    /// each frame with [`FoxControl::take_frame`].
    pub fn set_plane_mode(&self, enabled: bool) {
        self.plane_mode.set(enabled);
    }

    /// Whether frames go through the panel plane.
    pub fn plane_mode(&self) -> bool {
        self.plane_mode.get()
    }

    /// The canvas pool, allocating it on first use.
    pub(crate) fn pool(&self) -> &'static CanvasPool {
        if let Some(pool) = self.pool.get() {
            return pool;
        }
        let pool = CanvasPool::leak(FRAME_PIXELS);
        self.pool.set(Some(pool));
        pool
    }

    /// Take the newest rendered frame for the plane upload.
    ///
    /// Returns `None` when nothing new was rendered or the widget is not
    /// mounted. Dropping the lease returns the slot to the renderer.
    pub fn take_frame(&self) -> Option<ReadLease> {
        if !self.plane_mode.get() {
            return None;
        }
        self.pool.get()?.take_ready()
    }

    /// Publish a fully validated skin at one UI-owned switch point.
    pub fn set_skin(&self, skin: Rc<fox::skin::Skin>) {
        *self.skin.borrow_mut() = skin;
        self.set_loading(false);
        self.mark_urgent();
    }

    /// Keep recovery art hidden until the persisted selection is resolved.
    pub fn set_loading(&self, loading: bool) {
        if self.loading.replace(loading) != loading {
            self.mark_urgent();
        }
    }

    pub fn loading(&self) -> bool {
        self.loading.get()
    }

    /// Show a new expression. The mounted widget switches pose on the next
    /// [`rs_lvgl_ui::Ui::tick`]; no remount and no `App::view` call happen.
    pub fn set_expression(&self, expression: Expression) {
        self.expression.set(expression);
    }

    /// The expression the widget should currently display.
    pub fn expression(&self) -> Expression {
        self.expression.get()
    }

    /// Timed Lua animation selection; terminal runtime clears it before Sleep.
    pub fn set_play_pose(&self, pose: Option<FoxPose>) {
        if self.play_pose.replace(pose) != pose {
            self.mark_urgent();
        }
    }
    pub fn play_pose(&self) -> Option<FoxPose> {
        self.play_pose.get()
    }
    fn selected_pose(&self) -> FoxPose {
        self.play_pose
            .get()
            .unwrap_or_else(|| FoxPose::from(self.expression()))
    }

    /// Apply a new pose (squash, gaze, pulse).
    ///
    /// Only the rise of a tap pulse is urgent: that is the frame the finger's
    /// response lands in. The decay, eased gaze and squash ride the 50 ms
    /// cadence; marking every decay step urgent once rendered a frame every
    /// 5 ms for 280 ms and pushed touch P95 from 30 ms to 46 ms on device.
    pub fn set_visual(&self, visual: FoxVisual) {
        let visual = visual.sanitized();
        let previous = self.visual.replace(visual);
        if previous.pulse == 0.0 && visual.pulse > 0.0 {
            self.mark_urgent();
        }
    }

    /// The current pose.
    pub fn visual(&self) -> FoxVisual {
        self.visual.get()
    }

    /// Start the entrance drop from above the panel. The widget multiplies
    /// the entrance pose into the visual; touch is held off by the platform.
    pub fn start_entrance(&self) {
        self.entrance_elapsed_ms.set(0);
        self.entrance_active.set(true);
        self.entrance_finished.set(false);
    }

    /// Consume the "entrance just finished" edge.
    pub fn take_entrance_finished(&self) -> bool {
        self.entrance_finished.replace(false)
    }

    /// Whether the entrance animation is still running.
    pub fn entrance_active(&self) -> bool {
        self.entrance_active.get()
    }

    /// Render on the next tick, ignoring the 50 ms cadence.
    pub fn mark_urgent(&self) {
        self.urgent.set(true);
    }

    /// Number of successful `CustomWidget::create` calls observed.
    #[cfg(test)]
    pub fn create_count(&self) -> u32 {
        self.creates.get()
    }

    /// Number of `CustomWidget::delete` calls observed.
    #[cfg(test)]
    pub fn delete_count(&self) -> u32 {
        self.deletes.get()
    }

    /// True only when this tick published a real Sleep scene, not the old
    /// face retained by the animator during its expression swap.
    pub fn shutdown_sleep_frame_since(&self, before: u64) -> bool {
        self.rendered.get() > before
            && self.rendered_pose.get() == Some(FoxPose::Sleep)
            && !self.entrance_active()
    }

    /// Number of frames rendered and published.
    pub fn rendered_frames(&self) -> u64 {
        self.rendered.get()
    }

    fn bump(counter: &Cell<u64>) {
        counter.set(counter.get().saturating_add(1));
    }
}

/// LVGL canvas widget that renders the fox one frame at a time.
pub struct FoxWidget {
    control: Rc<FoxControl>,
    renderer: FoxRenderer,
    animator: FoxAnimator,
    /// Taken from the control in `create`, not in `new`, so descriptors built
    /// by an `App::view` the reconciler discards allocate nothing.
    pool: Option<&'static CanvasPool>,
    /// LVGL path only: the frame the native canvas currently displays.
    shown: Option<ReadLease>,
    last_expression: Option<Expression>,
    since_frame_ms: u32,
    /// Native canvas, kept so the LVGL path can repoint and invalidate it.
    canvas: Option<NonNull<lvgl_sys::lv_obj_t>>,
    status_canvas: Option<NonNull<lvgl_sys::lv_obj_t>>,
    #[cfg(target_os = "none")]
    render_budget: crate::platform::runtime::frequency::device::Budget,
}

impl FoxWidget {
    /// Create a widget descriptor bound to `control`. The canvas buffer is
    /// allocated when the descriptor is mounted.
    pub fn new(control: Rc<FoxControl>) -> Self {
        let pose = control.selected_pose();
        Self {
            control,
            renderer: FoxRenderer::new(),
            animator: FoxAnimator::new(pose),
            pool: None,
            shown: None,
            last_expression: None,
            since_frame_ms: 0,
            canvas: None,
            status_canvas: None,
            // Leave half the 50 ms frame interval for upload and scheduling.
            #[cfg(target_os = "none")]
            render_budget: crate::platform::runtime::frequency::device::Budget::new(25_000),
        }
    }

    /// The pose for this tick: personality visual × entrance × tap pulse.
    fn compose_input(&mut self, elapsed_ms: u32) -> FoxInput {
        let control = &self.control;
        let mut visual = control.visual();
        let mut offset_y = 0;
        if control.entrance_active() {
            let entrance_ms = control
                .entrance_elapsed_ms
                .get()
                .saturating_add(u64::from(elapsed_ms));
            control.entrance_elapsed_ms.set(entrance_ms);
            let pose = entrance::pose(entrance_ms);
            if entrance_ms >= entrance::TOTAL_MS {
                control.entrance_active.set(false);
                control.entrance_finished.set(true);
            }
            // The drop is drawn inside the canvas rather than moving it: the
            // plane uploads a fixed rectangle, so nothing is ever clipped at
            // the panel edge.
            offset_y = pose.offset_y;
            visual.scale_x *= pose.scale_x;
            visual.scale_y *= pose.scale_y;
        }
        if visual.pulse > 0.0 {
            visual.scale_x *= 1.0 + visual.pulse * 0.06;
            visual.scale_y *= 1.0 - visual.pulse * 0.04;
        }
        // A petting hand presses the head down a little and the fox leans
        // into the stroke; the warp keeps the bottom edge planted.
        if visual.pet > 0.0 {
            visual.scale_x *= 1.0 + visual.pet * 0.035;
            visual.scale_y *= 1.0 - visual.pet * 0.05;
        }
        let visual = visual.sanitized();
        FoxInput {
            scale_x: visual.scale_x,
            scale_y: visual.scale_y,
            offset_x: visual.lean_x as i32,
            offset_y,
            look_x: visual.look_x,
        }
    }

    /// LVGL path: release the previously shown slot, show the newest frame.
    fn show_latest(&mut self, pool: &'static CanvasPool, object: &mut CustomObject) {
        self.shown = None;
        self.shown = pool.take_ready();
        if let (Some(shown), Some(canvas), Some(status_canvas)) =
            (self.shown.as_ref(), self.canvas, self.status_canvas)
        {
            // SAFETY: `canvas` is the live native canvas created in `create`;
            // the pool buffer is `'static` and `self.shown` keeps the renderer
            // out of it until the next frame.
            unsafe {
                let pixels = shown.as_ptr().cast_mut();
                set_canvas_buffer(canvas, pixels, CANVAS_WIDTH, CANVAS_HEIGHT);
                set_canvas_buffer(
                    status_canvas,
                    pixels.add(fox::PIXELS),
                    status::WIDTH,
                    status::HEIGHT,
                );
                // This band sits outside its parent's character rectangle.
                lvgl_sys::lv_obj_invalidate(status_canvas.as_ptr());
            };
        }
        object.invalidate();
    }
}

/// Point `canvas` at a pool slot.
///
/// # Safety
///
/// `canvas` must be a live LVGL canvas and `pixels` a `'static` buffer of
/// `width * height` RGB565 pixels, protected by the displayed frame lease.
unsafe fn set_canvas_buffer(
    canvas: NonNull<lvgl_sys::lv_obj_t>,
    pixels: *mut u16,
    width: usize,
    height: usize,
) {
    // SAFETY: guaranteed by the caller.
    unsafe {
        lvgl_sys::lv_canvas_set_buffer(
            canvas.as_ptr(),
            pixels.cast(),
            width as i32,
            height as i32,
            lvgl_sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
        );
    }
}

mod native;

#[cfg(test)]
mod tests;
