//! `FoxWidget`: the fox-dango character as an LVGL custom widget.
//!
//! The widget renders 352x336 RGB565 frames with [`FoxRenderer`] into a
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
//! * `update` advances the [`FoxAnimator`] on every `Ui::tick` and renders on
//!   a 50 ms cadence, or on the next tick after an urgent change, into a pool
//!   [`WriteLease`](super::canvas_pool::WriteLease) and publishes it.
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

use super::canvas_pool::{CanvasPool, ReadLease};
use crate::behavior::Expression;
use crate::fox::{self, entrance, FoxAnimator, FoxInput, FoxPose, FoxRenderer};

/// Canvas width in pixels.
pub const CANVAS_WIDTH: usize = fox::W;
/// Canvas height in pixels.
pub const CANVAS_HEIGHT: usize = fox::H;

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

/// Monotonic microseconds for the widget's profiling counters.
///
/// The library is host-buildable and must not depend on `esp-hal`, so the
/// firmware installs its clock at startup. Without one the counters read zero,
/// which is what the host preview wants.
///
/// # Safety contract
///
/// `clock` must return monotonic microseconds and be callable from the UI task
/// without allocating or locking. The firmware passes
/// [`crate::telemetry::latency::now_us`].
pub fn install_mono_us(clock: fn() -> u64) {
    CLOCK.store(clock as usize, core::sync::atomic::Ordering::Release);
}

/// Installed clock, stored as a `fn() -> u64` pointer. Written once at startup.
static CLOCK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[inline]
fn mono_us() -> u64 {
    let raw = CLOCK.load(core::sync::atomic::Ordering::Acquire);
    if raw == 0 {
        return 0;
    }
    // SAFETY: `install_mono_us` stores exactly one `fn() -> u64` pointer, and a
    // non-zero value can only have come from it. Called once before the UI task
    // starts, so no torn or mismatched pointer is observable.
    let clock: fn() -> u64 = unsafe { core::mem::transmute(raw) };
    clock()
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
    visual: Cell<FoxVisual>,
    /// Render on the next tick instead of waiting for the 50 ms cadence.
    urgent: Cell<bool>,
    creates: Cell<u32>,
    deletes: Cell<u32>,
    updates: Cell<u64>,
    rendered: Cell<u64>,
    rendered_pose: Cell<Option<FoxPose>>,
    invalidations: Cell<u64>,
    entrance_active: Cell<bool>,
    entrance_elapsed_ms: Cell<u64>,
    entrance_finished: Cell<bool>,
    /// Microseconds spent advancing the animator on the last rendering tick.
    last_animate_us: Cell<u32>,
    /// Microseconds spent rendering the last frame.
    last_render_us: Cell<u32>,
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
            visual: Cell::new(FoxVisual::IDLE),
            urgent: Cell::new(true),
            creates: Cell::new(0),
            deletes: Cell::new(0),
            updates: Cell::new(0),
            rendered: Cell::new(0),
            rendered_pose: Cell::new(None),
            invalidations: Cell::new(0),
            entrance_active: Cell::new(false),
            entrance_elapsed_ms: Cell::new(0),
            entrance_finished: Cell::new(false),
            last_animate_us: Cell::new(0),
            last_render_us: Cell::new(0),
            plane_mode: Cell::new(false),
            pool: Cell::new(None),
        })
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
        let pool = CanvasPool::leak(CANVAS_WIDTH * CANVAS_HEIGHT);
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

    /// Frames the renderer produced that the upload never took.
    pub fn dropped_frames(&self) -> u32 {
        self.pool.get().map_or(0, CanvasPool::dropped_frames)
    }

    /// Publish a fully validated skin at one UI-owned switch point.
    pub fn set_skin(&self, skin: Rc<fox::skin::Skin>) {
        *self.skin.borrow_mut() = skin;
        self.mark_urgent();
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
    pub fn create_count(&self) -> u32 {
        self.creates.get()
    }

    /// Number of `CustomWidget::delete` calls observed.
    pub fn delete_count(&self) -> u32 {
        self.deletes.get()
    }

    /// Number of `CustomWidget::update` calls observed.
    pub fn update_count(&self) -> u64 {
        self.updates.get()
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

    /// Number of `CustomObject::invalidate` calls made by the widget (LVGL
    /// path only; one per rendered frame).
    pub fn invalidations(&self) -> u64 {
        self.invalidations.get()
    }

    /// Microseconds the animator spent advancing on the last rendering tick.
    pub fn last_animate_us(&self) -> u32 {
        self.last_animate_us.get()
    }

    /// Microseconds the renderer spent on the last frame.
    pub fn last_render_us(&self) -> u32 {
        self.last_render_us.get()
    }

    fn bump(counter: &Cell<u64>) {
        counter.set(counter.get().saturating_add(1));
    }
}

/// Elapsed microseconds since `started`, saturated to `u32`.
fn elapsed_us(started: u64) -> u32 {
    mono_us().saturating_sub(started).min(u64::from(u32::MAX)) as u32
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
        if let (Some(shown), Some(canvas)) = (self.shown.as_ref(), self.canvas) {
            // SAFETY: `canvas` is the live native canvas created in `create`;
            // the pool buffer is `'static` and `self.shown` keeps the renderer
            // out of it until the next frame.
            unsafe { set_canvas_buffer(canvas, shown.as_ptr().cast_mut()) };
        }
        object.invalidate();
        self.control.invalidations.set(object.invalidations());
    }
}

/// Point `canvas` at a pool slot.
///
/// # Safety
///
/// `canvas` must be a live LVGL canvas and `pixels` a `'static` buffer of
/// `CANVAS_WIDTH * CANVAS_HEIGHT` RGB565 pixels.
unsafe fn set_canvas_buffer(canvas: NonNull<lvgl_sys::lv_obj_t>, pixels: *mut u16) {
    // SAFETY: guaranteed by the caller.
    unsafe {
        lvgl_sys::lv_canvas_set_buffer(
            canvas.as_ptr(),
            pixels.cast(),
            CANVAS_WIDTH as i32,
            CANVAS_HEIGHT as i32,
            lvgl_sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
        );
    }
}

impl CustomWidget for FoxWidget {
    fn create(&mut self, parent: &mut Obj<'_>) -> Result<CustomObject, UiError> {
        if self.canvas.is_some() {
            return Err(UiError::InvalidProperty);
        }

        self.animator
            .set_skin(Rc::clone(&self.control.skin.borrow()));
        let expression = self.control.expression();
        self.animator.set_pose(self.control.selected_pose());
        self.last_expression = Some(expression);

        // Take (or allocate) the pool before creating the native object so an
        // allocation failure cannot leak a canvas. Publish one black frame so
        // the native canvas has valid pixels before the first update.
        let pool = self.control.pool();
        let plane = self.control.plane_mode();
        let mut first = pool.acquire().ok_or(UiError::Allocation)?;
        first.pixels_mut().fill(0);
        let first_ptr = first.pixels_mut().as_mut_ptr();
        first.publish();
        let shown = if plane {
            // The plane path never draws the native canvas; release the slot
            // so both slots are available for double buffering.
            drop(pool.take_ready());
            None
        } else {
            pool.take_ready()
        };

        // SAFETY: `parent` is a live LVGL object for the duration of `create`.
        let canvas = unsafe { lvgl_sys::lv_canvas_create(parent.raw().as_ptr()) };
        let Some(canvas) = NonNull::new(canvas) else {
            return Err(UiError::Allocation);
        };

        // SAFETY: `canvas` was just created as a child of `parent`. The pool
        // buffers are leaked and outlive the canvas. On the LVGL path `shown`
        // holds the read lease for `first_ptr`, so the renderer cannot write
        // into the displayed slot; on the plane path the canvas is hidden and
        // LVGL never reads its pixels.
        unsafe {
            set_canvas_buffer(canvas, first_ptr);
            // The size only settles on the next layout pass otherwise.
            lvgl_sys::lv_obj_set_size(canvas.as_ptr(), CANVAS_WIDTH as i32, CANVAS_HEIGHT as i32);
            lvgl_sys::lv_obj_center(canvas.as_ptr());
            // A canvas is not clickable by default; the custom node's
            // on_event handler needs real pointer events.
            lvgl_sys::lv_obj_set_clickable(canvas.as_ptr(), true);
            if plane {
                lvgl_sys::lv_obj_add_flag(
                    canvas.as_ptr(),
                    lvgl_sys::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN,
                );
            }
        }

        // SAFETY: `canvas` was just created as a direct child of `parent` and is live;
        // this is the only handle for it, it is returned from `create`, and
        // `delete` clears `self.canvas` before the reconciler frees it.
        // `unused_unsafe`: `CustomObject::new` became `unsafe` after the
        // pinned lv_binding_rust revision; this compiles against both.
        #[allow(unused_unsafe)]
        let object = unsafe { CustomObject::new(parent, canvas) };
        match object {
            Ok(object) => {
                self.pool = Some(pool);
                self.shown = shown;
                self.canvas = Some(canvas);
                self.control
                    .creates
                    .set(self.control.creates.get().saturating_add(1));
                // The first update renders the initial expression at once.
                self.control.mark_urgent();
                Ok(object)
            }
            Err(error) => {
                // Native objects created before an error are ours to delete.
                // SAFETY: the canvas is live, a direct child of `parent`, and
                // was never handed to the reconciler.
                unsafe { lvgl_sys::lv_obj_delete(canvas.as_ptr()) };
                Err(error)
            }
        }
    }

    fn update(&mut self, object: &mut CustomObject, elapsed_ms: u32) -> Result<(), UiError> {
        FoxControl::bump(&self.control.updates);
        let Some(pool) = self.pool else {
            return Err(UiError::InvalidProperty);
        };

        self.animator
            .set_skin(Rc::clone(&self.control.skin.borrow()));
        let expression = self.control.expression();
        if self.last_expression != Some(expression) {
            self.last_expression = Some(expression);
            self.animator.set_pose(self.control.selected_pose());
            // A new expression is the response the latency gate measures.
            self.control.mark_urgent();
        }

        self.animator.set_pose(self.control.selected_pose());

        // Advance on every tick so animation timing does not depend on the
        // render cadence.
        let animate_started = mono_us();
        self.animator.advance(elapsed_ms);
        let input = self.compose_input(elapsed_ms);
        self.control
            .last_animate_us
            .set(elapsed_us(animate_started));

        self.since_frame_ms = self.since_frame_ms.saturating_add(elapsed_ms);
        let frame_due = self.since_frame_ms >= FRAME_INTERVAL_MS;
        let urgent = self.control.urgent.replace(false);
        if !frame_due && !urgent {
            #[cfg(all(feature = "touch-latency-diagnostics", target_os = "none"))]
            crate::telemetry::touch_trace::with(|t| t.cadence_skip());
            return Ok(());
        }

        // Both slots busy (one on the wire, one shown or being written): keep
        // the frame due, and the urgency, for the next tick.
        let Some(mut lease) = pool.acquire() else {
            #[cfg(all(feature = "touch-latency-diagnostics", target_os = "none"))]
            crate::telemetry::touch_trace::with(|t| t.pool_miss());
            if urgent {
                self.control.mark_urgent();
            }
            return Ok(());
        };
        self.since_frame_ms = if frame_due {
            self.since_frame_ms % FRAME_INTERVAL_MS
        } else {
            0
        };

        let render_started = mono_us();
        let scene = self.animator.scene(input);
        self.renderer.render(&scene, lease.pixels_mut());
        self.control.last_render_us.set(elapsed_us(render_started));
        #[cfg(all(feature = "touch-latency-diagnostics", target_os = "none"))]
        crate::telemetry::touch_trace::with(|t| {
            t.raster(render_started, mono_us(), frame_due, urgent)
        });
        FoxControl::bump(&self.control.rendered);
        lease.publish();
        self.control.rendered_pose.set(Some(scene.pose));

        if !self.control.plane_mode() {
            self.show_latest(pool, object);
        }
        Ok(())
    }

    fn delete(&mut self, _object: &mut CustomObject) {
        self.control
            .deletes
            .set(self.control.deletes.get().saturating_add(1));
        self.canvas = None;
        // Nothing to free: the pool belongs to the control and is leaked, so
        // the native canvas (deleted by the reconciler after this hook) never
        // references freed pixels. Dropping `self.shown` with the widget
        // returns the displayed slot to the pool.
    }
}

#[cfg(test)]
mod tests {
    use rs_lvgl::Display;

    use super::*;
    use crate::test_support;

    /// Mount the widget on a 480x480 memory display and return the live UI.
    fn mount<'a>(control: &Rc<FoxControl>, display: &'a Display) -> rs_lvgl_ui::Ui<()> {
        struct App {
            control: Rc<FoxControl>,
        }
        impl rs_lvgl_ui::App for App {
            type Msg = ();
            fn update(&mut self, (): ()) {}
            fn view(&self, _theme: &rs_lvgl_ui::Theme) -> rs_lvgl_ui::View<()> {
                rs_lvgl_ui::custom(FoxWidget::new(Rc::clone(&self.control)))
            }
        }
        let app = App {
            control: Rc::clone(control),
        };
        let mut ui = rs_lvgl_ui::Ui::new(display, rs_lvgl_ui::Theme::dark()).expect("Ui::new");
        ui.mount(&app).expect("mount");
        ui
    }

    #[test]
    fn resident_lua_selects_pose_while_rust_renderer_keeps_animating_and_expires_it() {
        let _guard = test_support::lock();
        unsafe { rs_lvgl::deinit() };
        rs_lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
        let control = FoxControl::new(Expression::Idle);
        let mut runtime = crate::behavior::Runtime::new(0, Rc::clone(&control));
        runtime.enable_brain("local first=true;return {on_event=function()end,tick=function()fox.set_expression('Happy');if first then fox.play('Charging',500);first=false end end}",0,&lua_engine::GLOBAL_ALLOCATOR,None).unwrap();
        let mut ui = mount(&control, &display);
        for now in [50, 100, 150, 200] {
            runtime.tick(now);
            ui.tick(50).unwrap();
        }
        assert_eq!(control.rendered_pose.get(), Some(FoxPose::Charging));
        assert!(
            control.rendered_frames() >= 4,
            "Rust redraws independent animation frames between Lua decisions"
        );
        runtime.tick(550);
        ui.tick(50).unwrap();
        ui.tick(60).unwrap();
        assert_eq!(control.rendered_pose.get(), Some(FoxPose::Happy));
        runtime.begin_shutdown();
        ui.tick(5).unwrap();
        ui.tick(60).unwrap();
        assert_eq!(control.rendered_pose.get(), Some(FoxPose::Sleep));
        ui.unmount();
        drop(ui);
        unsafe {
            display.delete();
            rs_lvgl::deinit();
        }
    }

    #[test]
    fn a_scale_change_waits_for_the_cadence() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { rs_lvgl::deinit() };
        rs_lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
        let control = FoxControl::new(Expression::Idle);
        let mut ui = mount(&control, &display);
        ui.tick(50).expect("tick");
        let before = control.rendered_frames();

        let mut visual = control.visual();
        visual.scale_x = 1.05;
        control.set_visual(visual);
        ui.tick(5).expect("tick");
        assert_eq!(
            control.rendered_frames(),
            before,
            "eased gaze/scale rides the 50 ms cadence, not every loop"
        );
        ui.tick(FRAME_INTERVAL_MS).expect("tick");
        assert_eq!(
            control.rendered_frames(),
            before + 1,
            "the next animation frame draws the new shape"
        );

        ui.unmount();
        drop(ui);
        // SAFETY: the UI is unmounted and this test is the last LVGL user.
        unsafe { display.delete() };
        // SAFETY: the display is gone.
        unsafe { rs_lvgl::deinit() };
    }

    #[test]
    fn shutdown_receipt_rejects_the_old_face_during_the_sleep_swap() {
        let _guard = test_support::lock();
        unsafe { rs_lvgl::deinit() };
        rs_lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
        let control = FoxControl::new(Expression::Idle);
        let mut ui = mount(&control, &display);
        ui.tick(50).unwrap();
        control.set_expression(Expression::Sleep);
        let before = control.rendered_frames();
        ui.tick(5).unwrap();
        assert!(
            control.rendered_frames() > before,
            "new frame still contains the old pose during SWAP_MS"
        );
        assert_eq!(control.rendered_pose.get(), Some(FoxPose::Idle));
        assert!(!control.shutdown_sleep_frame_since(before));
        let before = control.rendered_frames();
        ui.tick(60).unwrap();
        assert_eq!(control.rendered_pose.get(), Some(FoxPose::Sleep));
        assert!(control.shutdown_sleep_frame_since(before));
        assert!(
            !control.shutdown_sleep_frame_since(control.rendered_frames()),
            "stale frame is not a receipt"
        );
        ui.unmount();
        drop(ui);
        unsafe {
            display.delete();
            rs_lvgl::deinit()
        };
    }

    #[test]
    fn default_brain_touch_ages_after_drains_and_publishes_an_urgent_frame() {
        let _guard = test_support::lock();
        unsafe { rs_lvgl::deinit() };
        rs_lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).unwrap();
        let control = FoxControl::new(Expression::Idle);
        control.set_plane_mode(true);
        let mut runtime = crate::behavior::Runtime::new(0, Rc::clone(&control));
        runtime
            .enable_brain(
                crate::behavior::brain::DEFAULT_SOURCE,
                0,
                &lua_engine::GLOBAL_ALLOCATOR,
                None,
            )
            .unwrap();
        let mut ui = mount(&control, &display);
        runtime.tick(1_000);
        ui.tick(50).unwrap();
        let baseline = control.take_frame().unwrap().pixels().to_vec();
        let before = control.rendered_frames();
        assert!(
            runtime.on_pointer(crate::behavior::personality::PointerSample {
                at_ms: 1_000,
                x: 320,
                y: 240,
                pressed: true,
            })
        );

        // The pre-drain timestamp leaves the real default brain's pulse at zero.
        runtime.tick(1_000);
        assert_eq!(control.visual().pulse, 0.0);
        ui.tick(5).unwrap();
        assert_eq!(control.rendered_frames(), before);
        assert!(control.take_frame().is_none());

        // A fresh timestamp includes time spent draining input. The existing
        // pulse-rise policy must publish actual pixels before the 50 ms cadence.
        runtime.tick(1_020);
        assert!(control.visual().pulse > 0.0);
        assert!(control.urgent.get());
        ui.tick(5).unwrap();
        assert_eq!(control.rendered_frames(), before + 1);
        let frame = control.take_frame().expect("touch response frame");
        assert_ne!(frame.pixels(), baseline.as_slice());
        drop(frame);
        ui.unmount();
        drop(ui);
        unsafe {
            display.delete();
            rs_lvgl::deinit();
        }
    }

    #[test]
    fn a_tap_pulse_renders_immediately() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { rs_lvgl::deinit() };
        rs_lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
        let control = FoxControl::new(Expression::Idle);
        let mut ui = mount(&control, &display);
        ui.tick(50).expect("tick");
        let before = control.rendered_frames();

        let mut visual = control.visual();
        visual.pulse = 0.5;
        control.set_visual(visual);
        ui.tick(5).expect("tick");
        assert_eq!(
            control.rendered_frames(),
            before + 1,
            "the tap thump rise must not wait for the animation cadence"
        );
        let after_rise = control.rendered_frames();

        // The decay must ride the cadence: rendering every 5 ms decay step
        // was a real device bug (P95 30 ms -> 46 ms).
        for step in 1..=5 {
            let mut visual = control.visual();
            visual.pulse = 0.5 - 0.05 * step as f32;
            control.set_visual(visual);
            ui.tick(5).expect("tick");
        }
        assert_eq!(
            control.rendered_frames(),
            after_rise,
            "pulse decay is animation and must not render every loop"
        );
        ui.tick(FRAME_INTERVAL_MS).expect("tick");
        assert_eq!(
            control.rendered_frames(),
            after_rise + 1,
            "the decay still draws on the animation cadence"
        );

        ui.unmount();
        drop(ui);
        // SAFETY: the UI is unmounted and this test is the last LVGL user.
        unsafe { display.delete() };
        // SAFETY: the display is gone.
        unsafe { rs_lvgl::deinit() };
    }

    #[test]
    fn the_entrance_finishes_and_reports_once() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { rs_lvgl::deinit() };
        rs_lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
        let control = FoxControl::new(Expression::Idle);
        let mut ui = mount(&control, &display);
        control.start_entrance();
        assert!(control.entrance_active());

        let mut elapsed = 0u32;
        while elapsed < crate::fox::entrance::TOTAL_MS as u32 + 100 {
            ui.tick(50).expect("tick");
            elapsed += 50;
        }
        assert!(!control.entrance_active(), "the entrance completed");
        assert!(
            control.take_entrance_finished(),
            "the finish edge is reported"
        );
        assert!(!control.take_entrance_finished(), "the edge is one-shot");

        ui.unmount();
        drop(ui);
        // SAFETY: the UI is unmounted and this test is the last LVGL user.
        unsafe { display.delete() };
        // SAFETY: the display is gone.
        unsafe { rs_lvgl::deinit() };
    }
}
