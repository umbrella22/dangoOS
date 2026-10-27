//! `SplashWidget`: the `ikaros` boot wordmark on an LVGL canvas.
//!
//! The widget owns one 320x120 RGB565 canvas (76.8 KiB, internal RAM) and
//! rasterizes the [`super`] timeline at a bounded cadence. It reports
//! completion through [`SplashControl`]; the application swaps the mounted tree
//! to the fox when that flag is set.
//!
//! The canvas is deliberately small: only the wordmark area is transferred, so
//! the splash costs a fraction of a full-screen frame and the first visible
//! frame after the bootloader splash arrives within one UI loop.

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::Cell;
use core::ptr::NonNull;

use rs_lvgl::sys as lvgl_sys;
use rs_lvgl_ui::{CustomObject, CustomWidget, NativeObject, Obj, UiError};

use super::{SPLASH_HEIGHT, SPLASH_PIXELS, SPLASH_WIDTH, TOTAL_MS};

/// Minimum interval between rasterized splash frames (20 fps).
pub const SPLASH_FRAME_INTERVAL_MS: u32 = 50;

/// Application-facing handle to the mounted [`SplashWidget`].
pub struct SplashControl {
    elapsed_ms: Cell<u64>,
    finished: Cell<bool>,
    frames: Cell<u32>,
    creates: Cell<u32>,
    updates: Cell<u64>,
    invalidations: Cell<u64>,
}

impl SplashControl {
    /// Create a control handle whose timeline starts at zero.
    ///
    /// Returns an `Rc` because the mounted widget and the application share the
    /// same handle; the UI task is single-threaded, so no atomics are needed.
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            elapsed_ms: Cell::new(0),
            finished: Cell::new(false),
            frames: Cell::new(0),
            creates: Cell::new(0),
            updates: Cell::new(0),
            invalidations: Cell::new(0),
        })
    }

    /// Timeline milliseconds consumed so far.
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms.get()
    }

    /// Whether the wordmark timeline has completed.
    pub fn finished(&self) -> bool {
        self.finished.get()
    }

    /// Number of rendered splash frames.
    pub fn frames(&self) -> u32 {
        self.frames.get()
    }

    /// Number of `create` calls, for lifecycle tests.
    pub fn create_count(&self) -> u32 {
        self.creates.get()
    }

    /// Number of `update` calls, for lifecycle tests.
    pub fn update_count(&self) -> u64 {
        self.updates.get()
    }

    /// Number of invalidations, which equals the rendered frames.
    pub fn invalidations(&self) -> u64 {
        self.invalidations.get()
    }

    /// Total timeline length in milliseconds.
    pub const fn total_ms(&self) -> u64 {
        TOTAL_MS
    }
}

/// LVGL canvas widget that renders the boot wordmark.
pub struct SplashWidget {
    control: Rc<SplashControl>,
    buffer: Option<Box<[u16; SPLASH_PIXELS]>>,
    since_frame_ms: u32,
    created: bool,
    last_painted: Option<u64>,
}

impl SplashWidget {
    /// Create a widget descriptor bound to `control`.
    pub fn new(control: Rc<SplashControl>) -> Self {
        Self {
            control,
            buffer: None,
            since_frame_ms: 0,
            created: false,
            last_painted: None,
        }
    }
}

impl CustomWidget for SplashWidget {
    fn create(&mut self, parent: &mut Obj<'_>) -> Result<CustomObject, UiError> {
        if self.created {
            return Err(UiError::InvalidProperty);
        }

        let mut buffer = Box::new([0u16; SPLASH_PIXELS]);

        // SAFETY: `parent` is a live LVGL object for the duration of `create`.
        let canvas = unsafe { lvgl_sys::lv_canvas_create(parent.raw().as_ptr()) };
        let Some(canvas) = NonNull::new(canvas) else {
            return Err(UiError::Allocation);
        };

        // SAFETY: the canvas was just created as a child of `parent`; `buffer`
        // is owned by `self` and outlives the canvas up to `delete`.
        unsafe {
            lvgl_sys::lv_canvas_set_buffer(
                canvas.as_ptr(),
                buffer.as_mut_ptr().cast(),
                SPLASH_WIDTH as i32,
                SPLASH_HEIGHT as i32,
                lvgl_sys::lv_color_format_t_LV_COLOR_FORMAT_RGB565,
            );
            lvgl_sys::lv_obj_set_size(canvas.as_ptr(), SPLASH_WIDTH as i32, SPLASH_HEIGHT as i32);
            lvgl_sys::lv_obj_center(canvas.as_ptr());
        }

        self.buffer = Some(buffer);
        self.created = true;
        self.control
            .creates
            .set(self.control.creates.get().saturating_add(1));

        // SAFETY: `canvas` was just created as a direct child of `parent` and is live;
        // this is the only handle for it, it is returned from `create`, and
        // the widget keeps no pointer to it.
        // `unused_unsafe`: `CustomObject::new` became `unsafe` after the
        // pinned lv_binding_rust revision; this compiles against both.
        #[allow(unused_unsafe)]
        let object = unsafe { CustomObject::new(parent, canvas) };
        match object {
            Ok(object) => Ok(object),
            Err(error) => {
                // SAFETY: the canvas is live and has not been handed to the
                // reconciler; the buffer is dropped with `self`.
                unsafe { lvgl_sys::lv_obj_delete(canvas.as_ptr()) };
                self.buffer = None;
                self.created = false;
                Err(error)
            }
        }
    }

    fn update(&mut self, object: &mut CustomObject, elapsed_ms: u32) -> Result<(), UiError> {
        self.control
            .updates
            .set(self.control.updates.get().saturating_add(1));
        let Some(buffer) = self.buffer.as_mut() else {
            return Err(UiError::InvalidProperty);
        };

        // The timeline follows wall-clock milliseconds so a slow frame cannot
        // stretch the splash; rendering is throttled independently.
        let elapsed = self
            .control
            .elapsed_ms
            .get()
            .saturating_add(u64::from(elapsed_ms));
        self.control.elapsed_ms.set(elapsed);

        self.since_frame_ms = self.since_frame_ms.saturating_add(elapsed_ms);
        let due = self.last_painted.is_none() || self.since_frame_ms >= SPLASH_FRAME_INTERVAL_MS;
        if !due {
            return Ok(());
        }
        self.since_frame_ms = 0;

        let finished = super::render(buffer.as_mut_slice(), elapsed);
        self.last_painted = Some(elapsed);
        self.control
            .frames
            .set(self.control.frames.get().saturating_add(1));
        object.invalidate();
        self.control
            .invalidations
            .set(self.control.invalidations.get().saturating_add(1));

        if finished {
            self.control.finished.set(true);
        }
        Ok(())
    }

    fn delete(&mut self, _object: &mut CustomObject) {
        self.created = false;
        // The boxed buffer is dropped with `self`; see the drop-order note in
        // `FoxWidget::delete`, which applies to every lifecycle widget.
    }
}

/// Canvas size in pixels, exported for the host preview.
pub const CANVAS_PIXELS: usize = SPLASH_PIXELS;
