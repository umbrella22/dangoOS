//! Lua 5.5 runtime task (RFC 0006 section 5, stage L0).
//!
//! L0 proves the interpreter on the device: a few seconds after boot it runs
//! a built-in self-test script, sliced into short steps so the UI keeps its
//! frame rate, and logs the output, the time it took and the Lua heap use.
//! Later stages turn this task into the app runner (display, delay, system,
//! json, storage, imu).

use alloc::{boxed::Box, string::String};
use core::alloc::{GlobalAlloc, Layout};

use embassy_time::{Duration, Instant, Timer};
use log::{info, warn};
use mosaico_lua::{Host, Limits, RawAllocator, Request, Step, Vm};

/// Lua heap on PSRAM.
unsafe fn psram_alloc(layout: Layout) -> *mut u8 {
    unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) }
}
unsafe fn psram_dealloc(ptr: *mut u8, layout: Layout) {
    unsafe { esp_alloc::HEAP.dealloc(ptr, layout) }
}
static PSRAM: RawAllocator = RawAllocator {
    alloc: psram_alloc,
    dealloc: psram_dealloc,
};

fn clock_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

/// Per-app limits: 4 MiB of PSRAM; 4 ms time slices checked every 200 VM
/// instructions; a script stuck 2 s where it cannot be yielded is stopped.
pub const LIMITS: Limits = Limits {
    memory_bytes: 4 * 1024 * 1024,
    hook_interval: 200,
    slice_hooks: 8,
    max_blocked_hooks: 20_000,
    clock: Some(clock_us),
    slice_us: 4_000,
    max_blocked_us: 2_000_000,
};

/// After this much Lua time without a sleep, the task sleeps 1 ms so timer
/// driven tasks (sensors) are not starved by back-to-back slices.
const BUSY_BUDGET_US: u64 = 16_000;

struct LogHost;

impl Host for LogHost {
    fn print(&mut self, line: &[u8]) {
        info!("lua> {}", String::from_utf8_lossy(line));
    }
}

fn log_sink(text: &[u8]) {
    let text = String::from_utf8_lossy(text);
    let text = text.trim_end();
    if !text.is_empty() {
        warn!("lua: {text}");
    }
}

const SELF_TEST: &str = r#"
print("hi from " .. _VERSION .. " on ESP-Mosaico")
local function lap(name, t) print(string.format("  %-8s %4d ms", name, os_clock_ms() - t)) return os_clock_ms() end
local t = os_clock_ms()
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local fib20 = fib(20)
t = lap("fib(20)", t)
local words = {}
for i = 1, 2000 do words[#words + 1] = string.format("%d:%.3f", i, math.sqrt(i)) end
t = lap("format", t)
table.sort(words)
t = lap("sort", t)
local acc = 0
for i = 1, 100000 do acc = acc + (i % 7) * 0.5 end
t = lap("loop", t)
local parts = {}
for i = 1, 5000 do parts[i] = { x = i, y = -i } end
parts = nil
collectgarbage()
t = lap("alloc+gc", t)
print(string.format("fib(20)=%d words=%d first=%s pi=%.10f int=%d float=%s acc=%g",
  fib20, #words, words[1], math.pi, math.maxinteger, tostring(2^53), acc))
print(string.format("utf8 %s len=%d, 0xff // 7 = %d", utf8.char(0x72d0, 0x72f8),
  utf8.len("\u{72d0}\u{72f8}"), 0xff // 7))
local ok, err = pcall(error, "caught")
print("pcall:", ok, err)
"#;

fn now_ms(call: &mut mosaico_lua::Call) -> Result<mosaico_lua::Ret, mosaico_lua::Error> {
    call.push_integer(Instant::now().as_millis() as i64);
    Ok(mosaico_lua::Ret::Values(1))
}

#[embassy_executor::task]
pub async fn lua_task() {
    mosaico_lua::set_log_sink(log_sink);
    // Let the splash and the first frames go out first.
    Timer::after(Duration::from_secs(4)).await;

    let free_before = esp_alloc::HEAP.free();
    let seed = Instant::now().as_ticks() as u32;
    let mut vm = match Vm::new(Box::new(LogHost), LIMITS, &PSRAM, seed) {
        Ok(vm) => vm,
        Err(e) => {
            warn!("lua: state creation failed: {e}");
            return;
        }
    };
    let state_bytes = vm.memory_used();
    if let Err(e) = vm.register("_G", &[("os_clock_ms", now_ms)]) {
        warn!("lua: register failed: {e}");
        return;
    }
    if let Err(e) = vm.load("selftest.lua", SELF_TEST.as_bytes()) {
        warn!("lua: {e}");
        return;
    }

    let started = Instant::now();
    let mut slices = 0u32;
    let mut longest_us = 0u64;
    let mut busy_us = 0u64;
    let outcome = loop {
        let slice_start = Instant::now();
        let step = vm.resume();
        let slice_us = slice_start.elapsed().as_micros();
        longest_us = longest_us.max(slice_us);
        busy_us += slice_us;
        slices += 1;
        match step {
            Step::Request(Request::Sleep { ms }) => {
                busy_us = 0;
                Timer::after_millis(ms as u64).await;
            }
            Step::Slice | Step::Request(_) if busy_us >= BUSY_BUDGET_US => {
                busy_us = 0;
                Timer::after_millis(1).await;
            }
            Step::Slice | Step::Request(_) => embassy_futures::yield_now().await,
            other => break other,
        }
    };
    let elapsed = started.elapsed().as_millis();
    match outcome {
        Step::Done => info!(
            "lua: L0 self-test ok in {elapsed} ms, {slices} slices (longest {longest_us} us), \
             state {state_bytes} B, peak {} B, now {} B",
            vm.memory_peak(),
            vm.memory_used()
        ),
        Step::Error(e) => warn!("lua: self-test failed: {e}"),
        other => warn!("lua: self-test ended with {other:?}"),
    }
    drop(vm);
    info!(
        "lua: heap free before {free_before} B, after close {} B",
        esp_alloc::HEAP.free()
    );
}
