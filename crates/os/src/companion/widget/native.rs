//! LVGL creation, frame publication and teardown.
use super::*;

impl CustomWidget for FoxWidget {
    // The shared control carries live runtime state; reconciliation has no
    // additional properties and must not advance the animation clock.
    type Props = ();

    fn create(&mut self, parent: &mut Obj<'_>, _: &()) -> Result<CustomObject, UiError> {
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
        // the native canvas has valid pixels before the first tick.
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
            set_canvas_buffer(canvas, first_ptr, CANVAS_WIDTH, CANVAS_HEIGHT);
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
        let object = unsafe { CustomObject::new(parent, canvas) };
        match object {
            Ok(object) => {
                self.pool = Some(pool);
                self.shown = shown;
                self.canvas = Some(canvas);
                #[cfg(test)]
                self.control
                    .creates
                    .set(self.control.creates.get().saturating_add(1));
                // The first tick renders the initial expression at once.
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

    fn apply_props(&mut self, _object: &mut CustomObject, _: &()) -> Result<(), UiError> {
        Ok(())
    }

    fn tick(&mut self, object: &mut CustomObject, elapsed_ms: u32) -> Result<(), UiError> {
        let Some(pool) = self.pool else {
            return Err(UiError::InvalidProperty);
        };

        self.animator
            .set_skin(Rc::clone(&self.control.skin.borrow()));
        let expression = self.control.expression();
        if self.last_expression != Some(expression) {
            self.last_expression = Some(expression);
            self.animator.set_pose(self.control.selected_pose());
            // A new expression renders on the next tick.
            self.control.mark_urgent();
        }

        self.animator.set_pose(self.control.selected_pose());

        // Advance on every tick so animation timing does not depend on the
        // render cadence.
        let input = if self.control.loading() {
            // The selected character gets its complete entrance after loading.
            FoxInput::REST
        } else {
            self.animator.advance(elapsed_ms);
            self.compose_input(elapsed_ms)
        };

        self.since_frame_ms = self.since_frame_ms.saturating_add(elapsed_ms);
        let frame_due = self.since_frame_ms >= FRAME_INTERVAL_MS;
        let urgent = self.control.urgent.replace(false);
        if !frame_due && !urgent {
            return Ok(());
        }

        // Both slots busy (one on the wire, one shown or being written): keep
        // the frame due, and the urgency, for the next tick.
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

        // The pool acquisition's critical section has already ended. This
        // synchronous scope runs on the sole LVGL owner, outside C callbacks;
        // no allocator, LVGL or global lock is held when requesting the clock.
        let wire_rgb565 = cfg!(all(target_os = "none", not(feature = "display-rgb888")))
            && self.control.plane_mode();
        let mut raster = || {
            let pixels = lease.pixels_mut();
            let pose = if self.control.loading() {
                if wire_rgb565 {
                    fox::loading::render_panel_rgb565be(pixels);
                } else {
                    fox::loading::render_panel(pixels);
                }
                None
            } else {
                let scene = self.animator.scene(input);
                if wire_rgb565 {
                    self.renderer.render_panel_rgb565be(&scene, pixels);
                } else {
                    self.renderer.render_panel(&scene, pixels);
                }
                Some(scene.pose)
            };
            if let Some(status) = self.control.radio_status.get() {
                if wire_rgb565 {
                    status::render_panel_rgb565be(pixels, status);
                } else {
                    status::render_panel(pixels, status);
                }
            }
            pose
        };
        #[cfg(target_os = "none")]
        let pose = self.render_budget.run(true, &mut raster);
        #[cfg(target_os = "none")]
        self.render_budget.finish();
        #[cfg(not(target_os = "none"))]
        let pose = raster();

        FoxControl::bump(&self.control.rendered);
        if wire_rgb565 {
            lease.publish_rgb565be();
        } else {
            lease.publish();
        }
        self.control.rendered_pose.set(pose);

        if !self.control.plane_mode() {
            self.show_latest(pool, object);
        }
        Ok(())
    }

    fn delete(&mut self, _object: &mut CustomObject) {
        #[cfg(test)]
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
