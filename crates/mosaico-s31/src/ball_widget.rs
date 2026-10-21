//! `BallWidget`: an LVGL canvas driven by the host-testable ball engine.
//!
//! The control selects the [`Character`]: the vector ball (240x240) or the
//! fox-dango sprites (352x336, [`crate::fox`]). The ball engine, entrance, tap
//! pulse and gaze drive both; only the pixels come from a different renderer.
//!
//! The widget renders 240x240 RGB565 frames into a two-slot
//! [`CanvasPool`](crate::canvas_pool::CanvasPool) at a bounded cadence. Application code controls the mounted instance through a
//! shared [`BallControl`] handle: `Node::custom` keeps the mounted widget when a
//! later `App::view` builds a new descriptor, so the handle is the supported
//! way to change the expression without remounting.
//!
//! # Sizing and quality (D-BALL-01)
//!
//! The first device run measured 14 fps with the 320x320 canvas and 2x2 edge
//! sampling, so this widget uses the documented fallback: a 240x240 canvas
//! (115,200 bytes of damage instead of 204,800) rendered with 2x1 edge
//! sampling (horizontal antialiasing, half the 2x2 scanline work). The
//! rasterizer scales through `size / 259`; the engine stays size-independent.
//!
//! # Lifecycle
//!
//! * `create` takes the control's canvas pool (allocated once per control and
//!   leaked, so its buffers are genuinely `'static`), creates the canvas as a
//!   direct child of the node's parent and centers it.
//! * `update` advances the engine on every `Ui::tick`, rasterizes on a 50 ms
//!   cadence (or immediately after a control change) into a pool
//!   [`WriteLease`](crate::canvas_pool::WriteLease) and publishes it.
//!   - LVGL path (host previews/tests): the widget itself takes the published
//!     frame, points the native canvas at it and invalidates it.
//!   - Plane path (device): the UI task takes the frame with
//!     [`BallControl::take_frame`] and hands the lease to the upload task. The
//!     native canvas is hidden and never drawn.
//! * `delete` only accounts the deletion: the pool outlives every widget, so
//!   no LVGL object can ever point at freed pixels.

use alloc::rc::Rc;
use core::cell::Cell;
use core::ptr::NonNull;

use grok_ball::engine::BallEngine;
use grok_ball::raster::{self, CanvasPose, DirtyRect, Quality};
use grok_ball::types::BallExpression;
use lvgl::sys as lvgl_sys;
use lvgl_ui::{CustomObject, CustomWidget, NativeObject, Obj, UiError};

use crate::canvas_pool::{CanvasPool, ReadLease};
use crate::entrance;
use crate::fox::{FoxAnimator, FoxInput, FoxPose, FoxRenderer};

/// Which character the widget draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Character {
    /// The procedural grok-ball vector rasterizer (240x240).
    Ball,
    /// The fox-dango palette sprites (352x336, see [`crate::fox`]).
    Fox,
}

impl Character {
    /// Canvas width and height in pixels.
    pub const fn canvas_size(self) -> (usize, usize) {
        match self {
            Self::Ball => (CANVAS_SIZE, CANVAS_SIZE),
            Self::Fox => (crate::fox::W, crate::fox::H),
        }
    }
}

/// Canvas edge in pixels (D-BALL-01 fallback; the rasterizer supports 320).
pub const CANVAS_SIZE: usize = 240;

/// Canvas pixel count.
pub const CANVAS_PIXELS: usize = CANVAS_SIZE * CANVAS_SIZE;

/// Edge sampling used on the device.
///
/// 2x1 keeps horizontal antialiasing on the slanted eye outlines while halving
/// the supersampling work of 2x2.
pub const QUALITY: Quality = Quality::TwoByOne;

/// Minimum interval between rasterized frames: 20 fps.
///
/// Engine state still advances on every `Ui::tick`; this only bounds the
/// software rasterizer, the canvas invalidation and the QSPI load. A pose or
/// expression change marks the control dirty and rasterizes immediately, so the
/// 50 ms cadence never delays a touch response. The plan accepts 20-25 fps.
pub const FRAME_INTERVAL_MS: u32 = 50;

/// How far the pose may scale the ball above its normal size.
///
/// The blob fills roughly 88% of the canvas, so anything beyond `1.10` starts
/// clipping the silhouette against its own 240x240 buffer. The widget clamps
/// instead of letting an animation cut the ball.
const MAX_POSE_SCALE: f32 = 1.10;
/// The per-frame pose applied on top of the engine's expression.
///
/// The expression and the pose are separate fields on purpose: the entrance
/// animates the pose with the Idle face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BallVisual {
    pub scale_x: f32,
    pub scale_y: f32,
    /// Extra gaze, in source units, added to both eyes' `look_x/look_y`.
    pub look_x: f32,
    pub look_y: f32,
    /// Decaying tap pulse, `0..=1`.
    pub pulse: f32,
}

impl BallVisual {
    /// Centered, unscaled, no extra gaze.
    pub const IDLE: Self = Self {
        scale_x: 1.0,
        scale_y: 1.0,
        look_x: 0.0,
        look_y: 0.0,
        pulse: 0.0,
    };

    /// Clamp into the range the canvas and the layout can serve.
    fn sanitized(self) -> Self {
        Self {
            scale_x: finite_or(self.scale_x, 1.0).clamp(0.4, MAX_POSE_SCALE),
            scale_y: finite_or(self.scale_y, 1.0).clamp(0.4, MAX_POSE_SCALE),
            look_x: finite_or(self.look_x, 0.0).clamp(-10.0, 10.0),
            look_y: finite_or(self.look_y, 0.0).clamp(-10.0, 10.0),
            pulse: finite_or(self.pulse, 0.0).clamp(0.0, 1.0),
        }
    }
}

impl Default for BallVisual {
    fn default() -> Self {
        Self::IDLE
    }
}

/// Monotonic microseconds for the widget's own profiling counters.
///
/// The library is host-buildable and must not depend on `esp-hal`, so the
/// firmware installs its clock at startup. Without one the counters read zero,
/// which is what the host preview wants.
///
/// # Safety contract
///
/// `clock` must return monotonic microseconds and be callable from the UI task
/// without allocating or locking. The firmware passes `crate::latency::now_us`.
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

/// Application-facing handle to the mounted [`BallWidget`].
///
/// Both the application and the mounted widget hold an `Rc` clone. The UI task
/// is single-threaded, so `Cell` is sufficient and no `Send`/`Sync` bound is
/// added.
pub struct BallControl {
    expression: Cell<BallExpression>,
    visual: Cell<BallVisual>,
    skin: Cell<&'static grok_ball::skin::Skin>,
    dirty: Cell<bool>,
    /// Urgent redraw: ignore the 50 ms animation cadence.
    urgent: Cell<bool>,
    creates: Cell<u32>,
    deletes: Cell<u32>,
    updates: Cell<u64>,
    rendered: Cell<u64>,
    invalidations: Cell<u64>,
    last_dirty_rect: Cell<Option<DirtyRect>>,
    entrance_active: Cell<bool>,
    entrance_elapsed_ms: Cell<u64>,
    entrance_finished: Cell<bool>,
    /// Microseconds spent in the engine `advance` on the last rasterizing tick.
    last_engine_us: Cell<u32>,
    /// Microseconds spent in the software rasterizer on the last tick.
    last_raster_us: Cell<u32>,
    /// When true, the LVGL object is hidden and never invalidated; the caller
    /// uploads frames through the dedicated ball plane instead.
    plane_mode: Cell<bool>,
    /// Canvas pool shared by every widget mounted on this control. Allocated
    /// on the first mount and never freed.
    pool: Cell<Option<&'static CanvasPool>>,
    /// Character drawn by the widget; fixed once the pool is allocated.
    character: Cell<Character>,
}

impl BallControl {
    /// Create a control handle showing `expression`.
    pub fn new(expression: BallExpression) -> Rc<Self> {
        Rc::new(Self {
            expression: Cell::new(expression),
            visual: Cell::new(BallVisual::IDLE),
            skin: Cell::new(&grok_ball::skin::BLOB),
            dirty: Cell::new(true),
            urgent: Cell::new(true),
            creates: Cell::new(0),
            deletes: Cell::new(0),
            updates: Cell::new(0),
            rendered: Cell::new(0),
            invalidations: Cell::new(0),
            last_dirty_rect: Cell::new(None),
            entrance_active: Cell::new(false),
            entrance_elapsed_ms: Cell::new(0),
            entrance_finished: Cell::new(false),
            last_engine_us: Cell::new(0),
            last_raster_us: Cell::new(0),
            plane_mode: Cell::new(false),
            pool: Cell::new(None),
            character: Cell::new(Character::Ball),
        })
    }

    /// Route the ball through the dedicated plane instead of LVGL.
    ///
    /// Must be set before the widget is mounted. Frames still render through
    /// [`BallWidget`], but the native canvas is hidden; the UI task collects
    /// each frame with [`BallControl::take_frame`].
    pub fn set_plane_mode(&self, enabled: bool) {
        self.plane_mode.set(enabled);
    }

    /// Whether the ball is uploaded through the dedicated plane.
    pub fn plane_mode(&self) -> bool {
        self.plane_mode.get()
    }

    /// Select the character. Must be called before the widget is mounted:
    /// the canvas pool is sized for the character on first mount.
    pub fn set_character(&self, character: Character) {
        if self.pool.get().is_none() {
            self.character.set(character);
            self.mark_urgent();
        }
    }

    /// The character the widget draws.
    pub fn character(&self) -> Character {
        self.character.get()
    }

    /// Canvas width and height of the current character.
    pub fn canvas_size(&self) -> (usize, usize) {
        self.character.get().canvas_size()
    }

    /// The canvas pool, allocating it on first use.
    pub(crate) fn pool(&self) -> &'static CanvasPool {
        if let Some(pool) = self.pool.get() {
            return pool;
        }
        let (w, h) = self.canvas_size();
        let pool = CanvasPool::leak(w * h);
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

    /// Select a new expression and request an immediate redraw.
    ///
    /// The mounted widget observes the change on the next [`lvgl_ui::Ui::tick`]
    /// and feeds it to [`BallEngine::set_expression`]. No remount and no
    /// `App::view` call are involved.
    pub fn set_expression(&self, expression: BallExpression) {
        if self.expression.get() != expression {
            self.expression.set(expression);
            self.mark_dirty();
        }
    }

    /// The expression the widget should currently display.
    pub fn expression(&self) -> BallExpression {
        self.expression.get()
    }

    /// Apply a new pose (drag offset, squash, gaze, pulse).
    ///
    /// A pure offset change only moves the native canvas and does not request
    /// a new bitmap: dragging at 5 ms cadence must not trigger a 13 ms raster
    /// per pointer sample. Scale, gaze and pulse changes do change pixels and
    /// set the redraw flag.
    pub fn set_visual(&self, visual: BallVisual) {
        let visual = visual.sanitized();
        let previous = self.visual.get();
        if previous == visual {
            return;
        }
        self.visual.set(visual);

        if previous.pulse == 0.0 && visual.pulse > 0.0 {
            // Only the *rise* of the tap thump is urgent: that is the frame the
            // finger's response lands in. The decay is animation and rides the
            // 50 ms cadence — marking every decay step urgent made the widget
            // rasterize a 240x240 frame every 5 ms for 280 ms, which starved
            // the UI loop and pushed P95 from 30 ms to 46 ms on device.
            self.mark_urgent();
        } else if previous.pulse != visual.pulse
            || previous.scale_x != visual.scale_x
            || previous.scale_y != visual.scale_y
            || previous.look_x != visual.look_x
            || previous.look_y != visual.look_y
        {
            // Gaze, squash and the pulse decay ease every loop; riding the
            // 50 ms cadence keeps a 13 ms raster from running 12 times a
            // second for motion nobody can see frame by frame.
            self.mark_dirty();
        }
    }

    /// The current pose.
    pub fn visual(&self) -> BallVisual {
        self.visual.get()
    }

    /// Select the character skin.
    ///
    /// The skin is a `&'static` table, so switching it costs one pointer and a
    /// redraw; it never remounts the widget.
    pub fn set_skin(&self, skin: &'static grok_ball::skin::Skin) {
        if !core::ptr::eq(self.skin.get(), skin) {
            self.skin.set(skin);
            self.mark_urgent();
        }
    }

    /// The current skin.
    pub fn skin(&self) -> &'static grok_ball::skin::Skin {
        self.skin.get()
    }

    /// Start the entrance drop from above the panel.
    ///
    /// The widget multiplies the entrance pose into the visual, so the caller
    /// only has to start it once; touch is held off by the platform.
    pub fn start_entrance(&self) {
        self.entrance_elapsed_ms.set(0);
        self.entrance_active.set(true);
        self.entrance_finished.set(false);
        self.mark_dirty();
    }

    /// Consume the "entrance just finished" edge.
    pub fn take_entrance_finished(&self) -> bool {
        self.entrance_finished.replace(false)
    }

    /// Whether the entrance animation is still running.
    pub fn entrance_active(&self) -> bool {
        self.entrance_active.get()
    }

    /// Request that the next widget update rasterizes and invalidates.
    pub fn mark_dirty(&self) {
        self.dirty.set(true);
    }

    /// Request an immediate raster, ignoring the 50 ms animation cadence.
    ///
    /// Used by expression changes, skin changes and the tap pulse: those are
    /// the responses the latency gate measures.
    pub fn mark_urgent(&self) {
        self.urgent.set(true);
    }

    /// Consume the pending urgent-redraw request.
    pub fn take_urgent(&self) -> bool {
        self.urgent.replace(false)
    }

    /// Consume the pending redraw request.
    pub fn take_dirty(&self) -> bool {
        self.dirty.replace(false)
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

    /// Number of frames the rasterizer produced and pushed to the canvas.
    pub fn rendered_frames(&self) -> u64 {
        self.rendered.get()
    }

    /// Number of `CustomObject::invalidate` calls made by the widget.
    ///
    /// This equals [`BallControl::rendered_frames`]: the widget invalidates
    /// only after a frame was rendered.
    pub fn invalidations(&self) -> u64 {
        self.invalidations.get()
    }

    /// Dirty rectangle returned by the most recent sized raster call.
    pub fn last_dirty_rect(&self) -> Option<DirtyRect> {
        self.last_dirty_rect.get()
    }

    /// Microseconds the engine spent advancing on the last rasterizing tick.
    pub fn last_engine_us(&self) -> u32 {
        self.last_engine_us.get()
    }

    /// Microseconds the software rasterizer spent on the last tick.
    pub fn last_raster_us(&self) -> u32 {
        self.last_raster_us.get()
    }

    fn record_create(&self) {
        self.creates.set(self.creates.get().saturating_add(1));
    }

    fn record_delete(&self) {
        self.deletes.set(self.deletes.get().saturating_add(1));
    }

    fn record_update(&self) {
        self.updates.set(self.updates.get().saturating_add(1));
    }

    fn record_render(&self, dirty: DirtyRect) {
        self.rendered.set(self.rendered.get().saturating_add(1));
        self.last_dirty_rect.set(Some(dirty));
    }

    fn record_invalidation(&self, invalidations: u64) {
        self.invalidations.set(invalidations);
    }
}

/// LVGL canvas widget that renders one [`BallEngine`] frame at a time.
pub struct BallWidget {
    engine: BallEngine,
    control: Rc<BallControl>,
    /// Pool taken from the control in `create`, not in `new`, so descriptors
    /// built by an `App::view` that the reconciler discards allocate nothing.
    pool: Option<&'static CanvasPool>,
    /// LVGL path only: the frame the native canvas currently displays.
    shown: Option<ReadLease>,
    last_expression: Option<BallExpression>,
    since_frame_ms: u32,
    created: bool,
    /// Canvas object, kept so the widget can invalidate it on the LVGL path.
    canvas: Option<NonNull<lvgl_sys::lv_obj_t>>,
    /// Fox renderer and expression player, created on mount for
    /// [`Character::Fox`].
    fox: Option<(FoxRenderer, FoxAnimator)>,
}

impl BallWidget {
    /// Create a widget descriptor bound to `control`.
    ///
    /// The canvas buffer is allocated when the descriptor is mounted.
    pub fn new(control: Rc<BallControl>) -> Self {
        Self {
            engine: BallEngine::new(),
            control,
            pool: None,
            shown: None,
            last_expression: None,
            since_frame_ms: 0,
            created: false,
            canvas: None,
            fox: None,
        }
    }
}

impl CustomWidget for BallWidget {
    fn create(&mut self, parent: &mut Obj<'_>) -> Result<CustomObject, UiError> {
        if self.created {
            return Err(UiError::InvalidProperty);
        }

        let expression = self.control.expression();
        self.engine.set_expression(expression);
        self.last_expression = Some(expression);
        let (canvas_w, canvas_h) = self.control.canvas_size();
        if self.control.character() == Character::Fox {
            self.fox = Some((FoxRenderer::new(), FoxAnimator::new(FoxPose::from(expression))));
        }

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
        // buffers are leaked and therefore outlive the native canvas. On the
        // LVGL path `shown` holds the read lease for `first_ptr`, so the
        // renderer cannot write into the displayed slot; on the plane path the
        // canvas is hidden below and LVGL never reads its pixels.
        unsafe {
            lvgl_sys::lv_canvas_set_buffer(
                canvas.as_ptr(),
                first_ptr.cast(),
                canvas_w as i32,
                canvas_h as i32,
                lvgl_sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
            );
            // The image class derives its size from the source, but the object
            // coordinates only settle on the next layout pass. Set the size
            // explicitly so the canvas is CANVAS_SIZE regardless of decoder
            // behavior; the alignment style is applied during the same layout
            // pass that positions the object.
            lvgl_sys::lv_obj_set_size(canvas.as_ptr(), canvas_w as i32, canvas_h as i32);
            lvgl_sys::lv_obj_center(canvas.as_ptr());
            // A canvas is an image object and not clickable by default. The
            // custom node's on_event handler only receives real pointer events
            // when the canvas is clickable.
            lvgl_sys::lv_obj_set_clickable(canvas.as_ptr(), true);
            if plane {
                lvgl_sys::lv_obj_add_flag(canvas.as_ptr(), lvgl_sys::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            }
        }

        self.pool = Some(pool);
        self.shown = shown;
        self.created = true;
        self.canvas = Some(canvas);
        self.control.record_create();
        // The canvas starts black and the first update renders the initial
        // expression immediately.
        self.control.mark_dirty();

        match CustomObject::new(parent, canvas) {
            Ok(object) => Ok(object),
            Err(error) => {
                // Keep the trait contract: native objects created before an
                // error are the implementation's responsibility.
                // SAFETY: the canvas is live, is a direct child of `parent`,
                // and has not been handed to the reconciler.
                unsafe { lvgl_sys::lv_obj_delete(canvas.as_ptr()) };
                self.pool = None;
                self.shown = None;
                self.canvas = None;
                self.created = false;
                Err(error)
            }
        }
    }

    fn update(&mut self, object: &mut CustomObject, elapsed_ms: u32) -> Result<(), UiError> {
        self.control.record_update();
        let (canvas_w, canvas_h) = self.control.canvas_size();

        let Some(pool) = self.pool else {
            return Err(UiError::InvalidProperty);
        };

        let expression = self.control.expression();
        if self.last_expression != Some(expression) {
            self.engine.set_expression(expression);
            self.last_expression = Some(expression);
            if let Some((_, animator)) = self.fox.as_mut() {
                animator.set_pose(FoxPose::from(expression));
            }
            // A new expression is the response the latency gate measures.
            self.control.mark_urgent();
        }

        // Advance the engine on every tick so animation timing is independent
        // of the raster cadence. `advance` clamps a single step to 1000 ms and
        // is a no-op for 0 ms.
        let engine_started = mono_us();
        let mut frame = self.engine.advance(elapsed_ms);
        if let Some((_, animator)) = self.fox.as_mut() {
            animator.advance(elapsed_ms);
        }
        self.control.last_engine_us.set(
            mono_us().saturating_sub(engine_started).min(u32::MAX as u64) as u32,
        );

        // Entrance owns the vertical drop and the landing squash; the
        // personality owns the drag offset, squirm and gaze. Both compose into
        // one `BallVisual` before rendering.
        let mut visual = self.control.visual();
        let entrance_scale;
        let mut entrance_offset_y = 0.0f32;
        if self.control.entrance_active() {
            let entrance_ms = self
                .control
                .entrance_elapsed_ms
                .get()
                .saturating_add(u64::from(elapsed_ms));
            self.control.entrance_elapsed_ms.set(entrance_ms);
            let pose = entrance::pose(entrance_ms);
            if entrance_ms >= entrance::TOTAL_MS {
                self.control.entrance_active.set(false);
                self.control.entrance_finished.set(true);
                self.control.mark_dirty();
            }
            // The drop is baked into the canvas bitmap, not a native object
            // move: the plane uploads a fixed 240x240 rectangle, so the ball
            // never crosses the panel edge and cannot leave a clipped strip.
            entrance_offset_y = pose.offset_y as f32;
            entrance_scale = (pose.scale_x, pose.scale_y);
        } else {
            entrance_scale = (1.0, 1.0);
        }
        visual.scale_x *= entrance_scale.0;
        visual.scale_y *= entrance_scale.1;

        // A tap pulse briefly widens the ball. It is a canvas pose, not an
        // engine expression, so it composes with every reaction.
        if visual.pulse > 0.0 {
            visual.scale_x *= 1.0 + visual.pulse * 0.06;
            visual.scale_y *= 1.0 - visual.pulse * 0.04;
        }
        let visual = visual.sanitized();

        // Extra gaze toward the finger is added to both eyes after the engine
        // composed the expression, which keeps every expression's own gaze.
        if visual.look_x != 0.0 || visual.look_y != 0.0 {
            for eye in [&mut frame.left, &mut frame.right] {
                eye.look_x += visual.look_x;
                eye.look_y += visual.look_y;
            }
        }

        self.since_frame_ms = self.since_frame_ms.saturating_add(elapsed_ms);
        let frame_due = self.since_frame_ms >= FRAME_INTERVAL_MS;
        let urgent = self.control.take_urgent();
        let _dirty = self.control.take_dirty();
        // `urgent` is the finger's direct feedback (expression, skin, tap
        // pulse) and draws on the next loop. Everything else — eased gaze,
        // entrance squash, engine animation — is continuous state and rides the
        // 50 ms cadence: a 13 ms raster per 5 ms loop would starve the UI.
        if !frame_due && !urgent {
            return Ok(());
        }

        // Both slots are busy (one on the wire, one being shown or written):
        // keep the frame due, and the urgency, for the next tick.
        let Some(mut lease) = pool.acquire() else {
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

        let pose = CanvasPose {
            scale_x: visual.scale_x,
            scale_y: visual.scale_y,
            // The bitmap stays centered; the in-canvas entrance is baked into
            // the pose, and the plane never moves the canvas rectangle.
            offset_x: 0.0,
            offset_y: entrance_offset_y,
            rotate_deg: 0.0,
        };
        let raster_started = mono_us();
        let rect = if let Some((renderer, animator)) = self.fox.as_mut() {
            let scene = animator.scene(FoxInput {
                scale_x: visual.scale_x,
                scale_y: visual.scale_y,
                offset_y: entrance_offset_y as i32,
                look_x: visual.look_x,
            });
            renderer.render(&scene, lease.pixels_mut());
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: (canvas_w - 1) as u16,
                y1: (canvas_h - 1) as u16,
            }
        } else {
            raster::render_sized_with_pose_and_skin(
                &frame,
                lease.pixels_mut(),
                CANVAS_SIZE,
                QUALITY,
                pose,
                self.control.skin(),
            )
        };
        self.control.last_raster_us.set(
            mono_us().saturating_sub(raster_started).min(u32::MAX as u64) as u32,
        );
        self.control.record_render(rect);
        lease.publish();
        if !self.control.plane_mode() {
            // Release the previously shown slot, then show the new frame.
            self.shown = None;
            self.shown = pool.take_ready();
            if let (Some(shown), Some(canvas)) = (self.shown.as_ref(), self.canvas) {
                // SAFETY: `canvas` is the live native canvas created in
                // `create`; the pool buffer is `'static` and `self.shown`
                // keeps the renderer out of it until the next frame.
                unsafe {
                    lvgl_sys::lv_canvas_set_buffer(
                        canvas.as_ptr(),
                        shown.as_ptr().cast_mut().cast(),
                        canvas_w as i32,
                        canvas_h as i32,
                        lvgl_sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
                    );
                }
            }
            object.invalidate();
            self.control.record_invalidation(object.invalidations());
        }
        Ok(())
    }

    fn delete(&mut self, _object: &mut CustomObject) {
        self.control.record_delete();
        self.created = false;
        self.canvas = None;

        // Nothing to free: the canvas pool belongs to the control and is
        // leaked, so the native canvas (deleted by the reconciler after this
        // hook) can never reference freed pixels. Dropping `self.shown` with
        // the widget returns the displayed slot to the pool.
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use lvgl::Display;

    use super::*;
    use crate::test_support;
    use grok_ball::types::BallExpression;

    /// Mount the widget on a 480x480 memory display and return the live UI.
    fn mount<'a>(
        control: &Rc<BallControl>,
        display: &'a Display,
    ) -> lvgl_ui::Ui<()> {
        struct App {
            control: Rc<BallControl>,
        }
        impl lvgl_ui::App for App {
            type Msg = ();
            fn update(&mut self, (): ()) {}
            fn view(&self, _theme: &lvgl_ui::Theme) -> lvgl_ui::Node<()> {
                lvgl_ui::Node::custom(BallWidget::new(Rc::clone(&self.control)))
            }
        }
        let app = App {
            control: Rc::clone(control),
        };
        let mut ui = lvgl_ui::Ui::new(display, lvgl_ui::Theme::dark()).expect("Ui::new");
        ui.mount(&app).expect("mount");
        ui
    }

    #[test]
    fn a_scale_change_rerasterizes() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { lvgl::deinit() };
        lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
        let control = BallControl::new(BallExpression::Idle);
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
        ui.tick(mosaico_s31::ball_widget::FRAME_INTERVAL_MS).expect("tick");
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
        unsafe { lvgl::deinit() };
    }

    #[test]
    fn a_tap_pulse_rasterizes_immediately() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { lvgl::deinit() };
        lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
        let control = BallControl::new(BallExpression::Idle);
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

        // The decay must ride the cadence: rasterizing every 5 ms decay step
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
            "pulse decay is animation and must not rasterize every loop"
        );
        ui.tick(mosaico_s31::ball_widget::FRAME_INTERVAL_MS).expect("tick");
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
        unsafe { lvgl::deinit() };
    }

    #[test]
    fn the_entrance_finishes_and_reports_once() {
        let _guard = test_support::lock();

        // SAFETY: the test lock is the only LVGL user in this test binary.
        unsafe { lvgl::deinit() };
        lvgl::init();
        let display = Display::register::<_, { 480 * 4 }>(480, 480, |_| {}).expect("display");
        let control = BallControl::new(BallExpression::Idle);
        let mut ui = mount(&control, &display);
        control.start_entrance();
        assert!(control.entrance_active());

        let mut elapsed = 0u32;
        while elapsed < crate::entrance::TOTAL_MS as u32 + 100 {
            ui.tick(50).expect("tick");
            elapsed += 50;
        }
        assert!(!control.entrance_active(), "the entrance completed");
        assert!(control.take_entrance_finished(), "the finish edge is reported");
        assert!(
            !control.take_entrance_finished(),
            "the edge is one-shot"
        );

        ui.unmount();
        drop(ui);
        // SAFETY: the UI is unmounted and this test is the last LVGL user.
        unsafe { display.delete() };
        // SAFETY: the display is gone.
        unsafe { lvgl::deinit() };
    }
}
