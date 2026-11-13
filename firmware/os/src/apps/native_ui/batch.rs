//! Bounded native commands with batch-local object references and creation rollback.
#[cfg(not(target_os = "none"))]
extern crate std;

use super::{values::*, NativeUiService};
use alloc::{format, string::String, vec::Vec};
use app_host::ui::{BATCH_COMMANDS_MAX, BATCH_OBJECTS_MAX, BATCH_SLICE_US};
use rs_lvgl::sys as s;
use serde_json::{json, Value};

enum Target {
    Handle(u64),
    Local(usize),
}

fn target(value: &Value, created: usize) -> Result<Target, String> {
    if let Some(id) = value.as_u64().filter(|id| *id > 0) {
        return Ok(Target::Handle(id));
    }
    if let Some(map) = value.as_object().filter(|map| map.len() == 1) {
        if let Some(index) = map.get("local").and_then(Value::as_u64) {
            if index < created as u64 {
                return Ok(Target::Local(index as usize));
            }
        }
    }
    Err("owned handle or previously created batch-local object required".into())
}

fn command(value: &Value) -> Result<(&str, &Vec<Value>), String> {
    let pair = value
        .as_array()
        .filter(|pair| pair.len() == 2)
        .ok_or("UI batch command requires [operation, arguments]")?;
    Ok((
        text(&pair[0])?,
        pair[1]
            .as_array()
            .ok_or("UI batch argument array required")?,
    ))
}

fn validate(args: &Value) -> Result<&Vec<Value>, String> {
    let outer = args
        .as_array()
        .filter(|args| args.len() == 1)
        .ok_or("UI batch requires one command array")?;
    let commands = outer[0]
        .as_array()
        .ok_or("UI batch command array required")?;
    if commands.len() > BATCH_COMMANDS_MAX {
        return Err("UI batch command limit exceeded".into());
    }
    let mut created = 0;
    for (index, value) in commands.iter().enumerate() {
        let validate_one = || -> Result<usize, String> {
            let (op, args) = command(value)?;
            let (arity, object_at, creates) = match op {
                "create" => (3, 1, true),
                "add_footer_button" => (2, 0, true),
                "set_size" | "set_pos" | "on" => (3, 0, false),
                "align" => (4, 0, false),
                "set_style" | "set_text" | "set_flex" | "set_scroll" | "add_flag"
                | "clear_flag" => (2, 0, false),
                "move_foreground" => (1, 0, false),
                _ => return Err(format!("unsupported UI batch operation: {op}")),
            };
            if args.len() != arity {
                return Err("invalid UI batch operation arity".into());
            }
            let object = target(&args[object_at], created)?;
            if op == "on" && !matches!(object, Target::Local(_)) {
                return Err("batch events require a newly created object".into());
            }
            if op == "create" {
                if !matches!(
                    text(&args[0])?,
                    "object" | "container" | "label" | "button" | "image" | "msgbox"
                ) {
                    return Err("unsupported batch widget kind".into());
                }
                let options = args[2].as_object().ok_or("batch options object required")?;
                if options.contains_key("src") || options.contains_key("color_format") {
                    return Err(
                        "file images and canvas allocation require separate UI requests".into(),
                    );
                }
            }
            Ok(usize::from(creates))
        };
        created += validate_one().map_err(|error| format!("UI batch command {index}: {error}"))?;
        if created > BATCH_OBJECTS_MAX {
            return Err("UI batch creation limit exceeded".into());
        }
    }
    Ok(commands)
}

pub(super) struct PendingBatch {
    commands: Vec<Value>,
    cursor: usize,
    created: Vec<u64>,
    checkpoint: u64,
    publish: bool,
    already_held: bool,
}

impl NativeUiService {
    pub(super) fn begin_batch(&mut self, args: Value, publish: bool) -> Result<(), String> {
        validate(&args)?;
        // Move the bounded, already parsed payload into the continuation.
        let Value::Array(mut outer) = args else {
            unreachable!()
        };
        let Some(Value::Array(commands)) = outer.pop() else {
            unreachable!()
        };
        let already_held = self.redraw.is_some();
        if !publish {
            self.begin_redraw();
        }
        self.pending = Some(PendingBatch {
            commands,
            cursor: 0,
            created: Vec::new(),
            checkpoint: self.next,
            publish,
            already_held,
        });
        Ok(())
    }

    pub(super) unsafe fn batch_slice(&mut self) -> Result<Option<Value>, String> {
        #[cfg(target_os = "none")]
        let started = crate::platform::runtime::resident_budget::clock_us();
        #[cfg(target_os = "none")]
        let elapsed =
            || crate::platform::runtime::resident_budget::clock_us().saturating_sub(started);
        #[cfg(not(target_os = "none"))]
        let started = std::time::Instant::now();
        #[cfg(not(target_os = "none"))]
        let elapsed = || started.elapsed().as_micros() as u64;
        unsafe { self.batch_slice_with_clock(elapsed) }
    }

    unsafe fn batch_slice_with_clock(
        &mut self,
        mut elapsed_us: impl FnMut() -> u64,
    ) -> Result<Option<Value>, String> {
        let mut batch = self.pending.take().ok_or("no UI batch pending")?;
        let result = (|| {
            while batch.cursor < batch.commands.len() {
                let index = batch.cursor;
                let (op, params) = command(&batch.commands[index])?;
                let mut params = params.clone();
                let at = usize::from(op == "create");
                let id = match target(&params[at], batch.created.len())? {
                    Target::Handle(id) => id,
                    Target::Local(index) => batch.created[index],
                };
                params[at] = json!(id);
                let reply = unsafe { self.dispatch(op, &Value::Array(params), Vec::new()) }
                    .map_err(|error| format!("UI batch command {index}: {error}"))?;
                if op == "create" || op == "add_footer_button" {
                    batch.created.push(uint(&reply[0])?);
                }
                batch.cursor += 1;
                // Always make progress, accept a completed request, and yield
                // only between commands. A cold LVGL call cannot be preempted.
                if batch.cursor < batch.commands.len() && elapsed_us() >= BATCH_SLICE_US {
                    return Ok(None);
                }
            }
            Ok(Some(json!(batch.created)))
        })();
        if matches!(&result, Ok(None)) {
            // The next owner turn can run timers, so hold intermediate geometry
            // and pointer hit testing until this same request finishes.
            self.begin_redraw();
            self.pending = Some(batch);
            return result;
        }
        if result.is_err() {
            // Includes a create whose options failed before its handle was returned.
            for (_, record) in self.objects.range(batch.checkpoint..) {
                if record.alive.get() {
                    unsafe { s::lv_obj_delete(record.ptr) };
                }
            }
        }
        if (result.is_ok() && batch.publish) || (result.is_err() && !batch.already_held) {
            self.finish_redraw();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::{cell::Cell, time::Duration};
    #[test]
    fn structural_errors_reject_forward_references_and_unbounded_work() {
        assert!(validate(&json!([[ ["create", ["label", {"local":0}, {}]] ]])).is_err());
        assert!(validate(&json!([[ ["create", ["image", 1, {"src":"D:/a.jpg"}]] ]])).is_err());
        assert!(validate(&json!([[["batch", [[]]]]])).is_err());
        assert!(validate(&json!([[["on", [1, "clicked", 1]]]])).is_err());
        let commands: Vec<_> = (0..BATCH_COMMANDS_MAX + 1)
            .map(|_| json!(["set_text", [1, "x"]]))
            .collect();
        assert!(validate(&json!([commands])).is_err());
        let commands: Vec<_> = (0..BATCH_OBJECTS_MAX + 1)
            .map(|_| json!(["create", ["label", 1, {}]]))
            .collect();
        assert!(validate(&json!([commands])).is_err());
    }

    fn call(ui: &mut NativeUiService, op: &str, args: Value) -> Value {
        let reply = ui
            .execute(app_host::ui::UiCommand {
                op: op.into(),
                args: args.to_string(),
                bytes: Vec::new(),
                root: "/data/test".into(),
            })
            .unwrap();
        serde_json::from_str(&reply.json).unwrap()
    }

    fn tick() {
        rs_lvgl::tick_inc(Duration::from_millis(20));
        unsafe {
            s::lv_timer_handler();
        }
    }

    fn with_ui(test: impl FnOnce(&mut NativeUiService, u64, Rc<Cell<usize>>)) {
        let _lock = crate::test_support::lock();
        rs_lvgl::init();
        let flushes = Rc::new(Cell::new(0));
        let count = flushes.clone();
        let display = rs_lvgl::Display::register::<_, { 80 * 8 }>(80, 80, move |_| {
            count.set(count.get() + 1)
        })
        .unwrap();
        let mut ui = NativeUiService::new(80, 80);
        call(&mut ui, "init", json!([{}]));
        let screen = call(&mut ui, "create", json!(["screen", 0, {}]))["values"][0]
            .as_u64()
            .unwrap();
        call(&mut ui, "load", json!([screen]));
        tick();
        test(&mut ui, screen, flushes);
        ui.release();
        assert_eq!(ui.object_count(), 0);
        drop(ui);
        unsafe { display.delete() };
    }

    #[test]
    fn slow_commands_yield_and_keep_local_handles_until_one_successful_reply() {
        with_ui(|ui, screen, flushes| {
            let before = flushes.get();
            ui.begin_batch(
                json!([[
                    ["create", ["button",screen,{}]],
                    ["on", [{"local":0},"clicked",41]],
                    ["create", ["label",{"local":0},{"text":"ready"}]],
                ]]),
                true,
            )
            .unwrap();
            assert!(unsafe { ui.batch_slice_with_clock(|| BATCH_SLICE_US) }
                .unwrap()
                .is_none());
            assert_eq!(ui.object_count(), 2);
            assert!(ui.has_pending());
            assert!(ui.redraw.is_some());
            tick();
            assert_eq!(
                flushes.get(),
                before,
                "a yielded batch must not publish intermediate geometry"
            );
            let busy = ui
                .submit(app_host::ui::UiCommand {
                    op: "events".into(),
                    args: "[]".into(),
                    bytes: Vec::new(),
                    root: "/other".into(),
                })
                .unwrap_err();
            assert!(busy.contains("already pending"));
            assert!(unsafe { ui.batch_slice_with_clock(|| BATCH_SLICE_US) }
                .unwrap()
                .is_none());
            let handles = unsafe { ui.batch_slice_with_clock(|| u64::MAX) }
                .unwrap()
                .unwrap();
            assert_eq!(handles.as_array().unwrap().len(), 2);
            assert!(!ui.has_pending());
            assert!(ui.redraw.is_none());
            tick();
            assert!(
                flushes.get() > before,
                "the final continuation publishes the completed tree"
            );
            let button = ui.get(handles[0].as_u64().unwrap()).unwrap().ptr;
            let label = ui.get(handles[1].as_u64().unwrap()).unwrap().ptr;
            unsafe {
                assert_eq!(s::lv_obj_get_parent(label), button);
                s::lv_obj_send_event(
                    button,
                    s::lv_event_code_t_LV_EVENT_CLICKED,
                    core::ptr::null_mut(),
                );
            }
            assert_eq!(call(ui, "events", json!([]))["values"], json!([41]));
        });
    }

    #[test]
    fn failure_in_a_later_slice_rolls_back_all_new_objects_and_preserves_existing_prefix() {
        with_ui(|ui, screen, _| {
            let label = call(ui, "create", json!(["label", screen, {"text":"before"}]))["values"]
                [0]
            .as_u64()
            .unwrap();
            for previous_gate in [false, true] {
                if previous_gate {
                    ui.begin_redraw();
                }
                let checkpoint = ui.next;
                ui.begin_batch(
                    json!([[
                        ["set_text", [label,"prefix applied"]],
                        ["create", ["button",screen,{}]],
                        ["on", [{"local":0},"clicked",41]],
                        ["create", ["label",{"local":0},{"bg_color":"invalid"}]],
                    ]]),
                    true,
                )
                .unwrap();
                for _ in 0..3 {
                    assert!(unsafe { ui.batch_slice_with_clock(|| BATCH_SLICE_US) }
                        .unwrap()
                        .is_none());
                }
                let error = unsafe { ui.batch_slice_with_clock(|| 0) }.unwrap_err();
                assert!(error.contains("UI batch command 3"));
                assert!(!ui.has_pending());
                assert_eq!(ui.object_count(), 2);
                assert_eq!(ui.redraw.is_some(), previous_gate);
                let reply = call(ui, "events", json!([]));
                assert_eq!(reply["retired"], json!([41]));
                assert_eq!(reply["values"], json!([]));
                assert!(ui.objects.range(checkpoint..).next().is_none());
                assert!(ui.next > checkpoint);
                unsafe {
                    assert_eq!(
                        core::ffi::CStr::from_ptr(s::lv_label_get_text(ui.get(label).unwrap().ptr))
                            .to_str()
                            .unwrap(),
                        "prefix applied"
                    );
                }
                ui.finish_redraw();
            }
        });
    }

    #[test]
    fn release_cancels_the_continuation_before_a_new_instance_acquires_the_service() {
        with_ui(|ui, screen, _| {
            ui.begin_batch(
                json!([[
                    ["create", ["button",screen,{}]],
                    ["on", [{"local":0},"clicked",41]],
                    ["create", ["label",{"local":0},{}]],
                ]]),
                false,
            )
            .unwrap();
            for _ in 0..2 {
                assert!(unsafe { ui.batch_slice_with_clock(|| BATCH_SLICE_US) }
                    .unwrap()
                    .is_none());
            }
            let next = ui.next;
            ui.release();
            assert!(!ui.has_pending());
            assert!(ui.redraw.is_none());
            assert_eq!(ui.object_count(), 0);
            assert!(ui.queue.borrow().is_empty());
            assert!(ui.resume().is_err());
            call(ui, "init", json!([{}]));
            let new_screen = call(ui, "create", json!(["screen", 0, {}]))["values"][0]
                .as_u64()
                .unwrap();
            assert!(new_screen >= next);
            assert!(ui.get(screen).is_err());
            assert_eq!(call(ui, "events", json!([]))["values"], json!([]));
        });
    }
}
