use crate::recorder::Recorder;
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc, sync::OnceLock, time::Instant};

type Shared = Rc<RefCell<Recorder>>;
const MEMORY_BYTES: usize = 4 * 1024 * 1024;
static EPOCH: OnceLock<Instant> = OnceLock::new();
fn clock_us() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

fn measured<T>(recorder: &Shared, action: impl FnOnce() -> T) -> T {
    recorder.borrow_mut().enter();
    let result = action();
    recorder.borrow_mut().leave();
    result
}

pub fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: script-bench-runner TRUSTED_SCRIPT ARGS.json".into());
    }
    let script = std::fs::read(&args[0]).map_err(|e| e.to_string())?;
    let config: Value =
        serde_json::from_slice(&std::fs::read(&args[1]).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let recorder = Rc::new(RefCell::new(Recorder::default()));
    let report = execute(&script, config, recorder.clone())?;
    let recorder = recorder.borrow();
    recorder.finish()?;
    let mut report = report;
    report["events"] = json!(recorder.events);
    report["max_vm_step_ns"] = json!(recorder.max_vm_step_ns);
    println!("{report}");
    Ok(())
}

#[cfg(feature = "lua")]
fn execute(script: &[u8], config: Value, recorder: Shared) -> Result<Value, String> {
    struct Host(Shared);
    impl lua_engine::Host for Host {
        fn print(&mut self, line: &[u8]) {
            self.0.borrow_mut().line(&String::from_utf8_lossy(line));
        }
    }
    let load = Instant::now();
    let limits = lua_engine::Limits {
        memory_bytes: MEMORY_BYTES,
        hook_interval: 200,
        clock: Some(clock_us),
        ..Default::default()
    };
    let mut vm = lua_engine::Vm::new(
        Box::new(Host(recorder.clone())),
        limits,
        &lua_engine::GLOBAL_ALLOCATOR,
        1,
    )?;
    vm.set_args_json(&config.to_string())?;
    vm.load("script-bench", script)?;
    let load_ns = load.elapsed().as_nanos() as u64;
    let running = Instant::now();
    loop {
        if running.elapsed().as_secs() > 300 {
            return Err("host benchmark timed out".into());
        }
        match measured(&recorder, || vm.resume()) {
            lua_engine::Step::Slice
            | lua_engine::Step::Request(lua_engine::Request::Sleep { ms: 0 }) => {}
            lua_engine::Step::Done => break,
            other => return Err(format!("unexpected Lua stop: {other:?}")),
        }
        if let Some(error) = &recorder.borrow().error {
            return Err(error.clone());
        }
    }
    let report = json!({
        "engine":"lua", "runtime":lua_engine::LUA_RELEASE,
        "load_ns":load_ns, "run_wall_ns":running.elapsed().as_nanos() as u64,
        "limits":{"memory_bytes":MEMORY_BYTES,"hook_interval":200,"slice_us":limits.slice_us},
        "memory":{"peak_bytes":vm.memory_peak(),"used_before_close_bytes":vm.memory_used(),
                  "used_after_close_bytes":Value::Null,"accounting":"Lua requested allocation sizes"}
    });
    drop(vm);
    Ok(report)
}

#[cfg(feature = "ts")]
fn execute(script: &[u8], config: Value, recorder: Shared) -> Result<Value, String> {
    struct Host {
        recorder: Shared,
        args: Value,
    }
    impl ts_engine::Host for Host {
        fn now_ms(&mut self) -> Result<u64, ts_engine::HostError> {
            Ok(clock_us() / 1000)
        }
        fn call(
            &mut self,
            operation: &str,
            args: &str,
            _: &[u8],
        ) -> Result<ts_engine::HostReply, ts_engine::HostError> {
            let value = match operation {
                "args" => self.args.clone(),
                "now_ms" => json!(clock_us() / 1000),
                "print" => {
                    let values: Vec<String> = serde_json::from_str(args)
                        .map_err(|e| ts_engine::HostError::new(-1, e.to_string()))?;
                    if values.len() != 1 {
                        return Err(ts_engine::HostError::new(-1, "one print argument required"));
                    }
                    self.recorder.borrow_mut().line(&values[0]);
                    Value::Null
                }
                _ => {
                    return Err(ts_engine::HostError::new(
                        -1,
                        format!("unexpected operation {operation}"),
                    ))
                }
            };
            Ok(ts_engine::HostReply::json(value.to_string()))
        }
    }
    let load = Instant::now();
    let limits = ts_engine::Limits {
        memory_bytes: MEMORY_BYTES,
        execution_us: 20_000,
        interrupt_polls: 200,
        clock: Some(clock_us),
        ..Default::default()
    };
    let mut vm = ts_engine::Vm::new(
        Box::new(Host {
            recorder: recorder.clone(),
            args: config,
        }),
        limits,
        &ts_engine::GLOBAL_ALLOCATOR,
    )
    .map_err(|e| e.to_string())?;
    let package = ts_engine::Package::parse(script).map_err(|e| e.to_string())?;
    // Only artifacts compiled from this local benchmark's trusted sources are accepted.
    unsafe { vm.load(package) }.map_err(|e| e.to_string())?;
    let load_ns = load.elapsed().as_nanos() as u64;
    let running = Instant::now();
    let mut state = measured(&recorder, || vm.start("main", None)).map_err(|e| e.to_string())?;
    loop {
        if running.elapsed().as_secs() > 300 {
            return Err("host benchmark timed out".into());
        }
        while let Some(queued) = vm.take_request() {
            let request = match queued {
                ts_engine::Request::Delay { id, micros: 0 } => {
                    measured(&recorder, || vm.complete_void(id)).map_err(|e| e.to_string())?;
                    continue;
                }
                ts_engine::Request::Host(request) => request,
                other => return Err(format!("unexpected request: {other:?}")),
            };
            if request.operation != "sleep" || request.args != "[0]" {
                return Err(format!(
                    "unexpected async operation: {} {}",
                    request.operation, request.args
                ));
            }
            measured(&recorder, || {
                vm.complete(request.id, Ok(ts_engine::HostReply::json("null")))
            })
            .map_err(|e| e.to_string())?;
        }
        if !vm.has_pending_jobs() && matches!(state, ts_engine::CallState::Done(_)) {
            break;
        }
        let jobs = measured(&recorder, || vm.drain_jobs(1)).map_err(|e| e.to_string())?;
        state = measured(&recorder, || vm.entry_status()).map_err(|e| e.to_string())?;
        if matches!(state, ts_engine::CallState::Missing)
            || (matches!(state, ts_engine::CallState::Pending)
                && !jobs.pending
                && vm.pending_requests() == 0)
        {
            return Err("TS entry missing or stalled".into());
        }
        if let Some(error) = &recorder.borrow().error {
            return Err(error.clone());
        }
    }
    let run_wall_ns = running.elapsed().as_nanos() as u64;
    let before = vm.memory_stats();
    let after = vm.close();
    if after.used != 0 || after.failures != 0 {
        return Err(format!("TS memory ledger: {after:?}"));
    }
    Ok(json!({
        "engine":"ts", "runtime":"QuickJS-NG", "engine_id":ts_engine::ENGINE_ID,
        "load_ns":load_ns, "run_wall_ns":run_wall_ns,
        "limits":{"memory_bytes":MEMORY_BYTES,"execution_us":limits.execution_us,"interrupt_polls":limits.interrupt_polls},
        "memory":{"peak_bytes":before.peak,"used_before_close_bytes":before.used,
                  "used_after_close_bytes":after.used,"allocations":after.allocations,"failures":after.failures,
                  "accounting":"Backing allocations including alignment headers and C wrapper"}
    }))
}
