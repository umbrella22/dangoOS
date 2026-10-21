# RFC 0005: Platform Runtime — esp-rtos/Embassy Tasks, Async Display and Console (Draft)

> Status: **Draft implementation plan; no implementation work package is complete.**
> Plan ID: `mosaico-platform-runtime-v1`; revision: `1`.
> Baseline date: `2026-10-20` UTC.
> Baseline: the validated single-threaded `mosaico-s31` prototype is archived at
> `archive/crates-pre-grok-ball-20261020/mosaico-s31`; the workspace contains the
> host-testable `grok-ball` crate.
> Related plans: [RFC 0003 Grok-Ball First Screen](grok-ball-first-screen-plan.md)
> (device integration waits for R1), [RFC 0004 Grok-Ball Sensor Reactions](grok-ball-sensor-reactions-plan.md)
> (device integration waits for R1/R1.5), [Execution order](ROADMAP.md).
> Implementation authorization: **not granted by authoring this RFC.**

Navigation: [Purpose](#1-purpose-and-runtime-rule) · [Baseline](#2-baseline) · [Architecture](#3-target-architecture) · [R0](#4-r0-task-skeleton-and-console) · [R1](#5-r1-async-display-and-input) · [R1.5](#6-r15-sensor-task) · [Entry points](#7-operational-entry-points) · [Acceptance](#8-testing-and-acceptance) · [Milestones](#9-milestones-and-estimates) · [Risks](#10-risks-and-open-decisions) · [Handoff](#11-handoff)

## 1. Purpose and runtime rule

RFC 0005 turns the R0/R1/R1.5 platform track from a footnote in RFC 0003 and
RFC 0004 into an executable plan. It replaces the archived single-threaded pump
with an `esp-rtos` + Embassy task model, an asynchronous QSPI presentation path,
an interrupt-driven touch input path and a sensor task.

The primary runtime rule:

**The UI task exclusively owns LVGL. The display task exclusively owns the panel
and the presentation buffers. The input task exclusively owns the touch
controller. The sensor task exclusively owns the sensor bus. No task calls LVGL
except the UI task, and no task blocks on a full queue.**

The second rule:

**Input capture never runs inside the LVGL flush callback. Display transfer never
runs on the UI task. Sensor polling never runs on the input task.**

This is the direct fix for the archived prototype's latency: the handler gate
`!port.is_pending()`, the blocking `SpiDma<Blocking>` write, the I2C sampling in
the flush callback and the single loop that serialized render and transfer.

## 2. Baseline

### 2.1 Archived prototype

The archived crate proves the hardware path but not the performance target:

| Item | Archived behavior | R0/R1 target |
| --- | --- | --- |
| Scheduler | One loop, handler gated on `!port.is_pending()` | `esp-rtos` + Embassy, independent tasks |
| Display | Blocking `SpiDma<Blocking>::half_duplex_write` in 16-row chunks | `SpiDma<Async>` + double-buffered presentation |
| Touch | 5 ms I2C polling; flush callback samples input | GPIO6 interrupt + async I2C reads on `input_task` |
| Framebuffer | One retained frame + 48-line LVGL draw buffer | Two presentation buffers, dirty-rectangle transfer |
| Console | No application USB CDC path on the tested board | USB-OTG HS CDC-ACM or UART0 established in R0 |
| Sensors | Not part of the prototype loop | `sensor_task` with a bounded event queue |

The archived drivers are the source of truth for the panel and touch
initialization. R0 ports them; it does not rewrite the register sequences.

### 2.2 Official reference

The official `esp-mosaico-claw` firmware uses:

- a FreeRTOS LVGL task with a 10 ms maximum period and a 5 ms tick;
- an `esp_lcd` presentation path with `trans_queue_depth = 2` and TE sync;
- GPIO6 interrupt-driven touch with cached points;
- a separate sensor path;
- GSP/PPA for the home scene, with PPA disabled for the LVGL adapter.

RFC 0005 copies the task separation, the interrupt-driven input and the
double-buffered presentation. It does not copy the GSP/PPA scene pipeline; the
grok-ball renderer is a software rasterizer into an LVGL canvas.

## 3. Target architecture

### 3.1 Tasks

| Task | Priority | Trigger | Owns | Must not |
| --- | --- | --- | --- | --- |
| `ui_task` | Medium | 10 ms timer and input events | LVGL, `Ui<Msg>`, `App`, ball widget, AffectEngine | Touch I2C, panel SPI, sensor I2C |
| `display_task` | High | `FrameReady` signal or TE | QSPI panel, presentation buffers, TE | LVGL objects, app state |
| `input_task` | High | GPIO6 falling edge, 5 ms fallback | CST9220 reads, input queue | LVGL, panel, sensor bus |
| `sensor_task` (R1.5) | Low | 20–50 Hz timer | BMI270, BMM150, right-slot module | LVGL, panel, touch reads |

Priority is expressed as high/medium/low rather than numeric values because
`esp-rtos` and Embassy use different conventions. The implementation records the
actual numeric priorities in the ledger.

### 3.2 Data flow

```text
CST9220 INT (GPIO6)
    -> input_task: read report 0xD000, ACK 0xAB, decode points
    -> InputEvent queue (bounded)
    -> ui_task: PointerPort / Ui::post / Ui::drain
    -> App::update -> App::view -> reconciler -> LVGL render
    -> LVGL flush callback: mark dirty rectangle, signal display_task
    -> display_task: CASET/RASET + QSPI chunk transfer into the panel
    -> TE / transfer-complete: release the front buffer

BMI270 / BMM150 / right-slot module
    -> sensor_task: sample, debounce, classify
    -> SensorEvent queue (bounded)
    -> ui_task: AffectEngine::on_event -> BallExpression
    -> BallWidget::update -> canvas invalidation -> LVGL flush
```

### 3.3 Buffers and presentation

- Panel: CO5300, 480×480, QSPI 4-lane, 40 MHz, opcode `0x32`.
- Two presentation buffers, 480×480 RGB565, 460,800 bytes each, PSRAM.
- LVGL draws into the back buffer. The display task transfers the front buffer.
- The flush callback records the union of dirty rectangles; the display task
  transfers only the dirty region, aligned to 4 pixels in x.
- When the transfer completes, the buffers swap and the released buffer becomes
  the new back buffer.
- R1a uses full-frame PSRAM buffers so LVGL can render directly into the back
  buffer without a memcpy.
- If PSRAM render bandwidth misses the frame budget, R1b falls back to two
  internal-RAM partial draw buffers (480×48) plus a PSRAM staging buffer and a
  tile memcpy. The choice is recorded in the ledger.

### 3.4 Input path

- GPIO6 is configured as a falling-edge interrupt. The handler only signals the
  input task; it performs no I2C access.
- `input_task` reads the CST9220 report, sends the HYN212 ACK, decodes the
  points and pushes an `InputEvent` into a bounded queue (capacity 32).
- A 5 ms timer polls the controller as a missed-interrupt fallback. Polling and
  interrupt reads share one async I2C mutex.
- Coordinates map to LVGL through the existing `PointerPort`/`TouchInput` logic.
- The LVGL flush callback no longer samples the touch controller.

### 3.5 Sensor path (R1.5)

- A separate `sensor_task` owns BMI270 (`0x68`), BMM150 (`0x22`/`0x24`) and the
  right-slot Interaction module (`EEPROM 0x51`, `board_type 0x16`).
- The shared I2C0 bus is protected by an async mutex. Touch reads have priority;
  sensor transactions use a short timeout and yield on contention.
- `sensor_task` publishes `SensorEvent` values through a bounded queue
  (capacity 64). On overflow it drops the oldest non-edge event and increments a
  counter. Edge events (contact, release, insert, remove, key press) are never
  dropped silently; the counter is reported in the stats line.
- The affect engine runs on `ui_task`; `sensor_task` never selects an expression.

### 3.6 Console and diagnostics

The tested board's USB-C connector is wired to the **USB-OTG HS** controller,
not to the USB-Serial-JTAG peripheral. The official firmware enumerates a
TinyUSB CDC-ACM console on USB-OTG. R0 uses the esp-hal USB HS driver
(`Usb::new_hs(peripherals.USB_HS)`) with `embassy-usb` CDC-ACM and a bounded
log bridge. `esp-println` with the `jtag-serial` feature does **not** enumerate
on this connector; UART0 pads with a 3.3 V adapter are the fallback.

The console must print a startup banner, task heartbeats, queue overflow counts,
flush counts and transfer times. R1 adds the P95 instrumentation counters.

## 4. R0 task skeleton and console

### Scope

- Create `crates/mosaico-s31` from the archived crate.
- Add `esp-rtos` and the Embassy dependencies.
- Port the archived display, touch, shared-I2C and framebuffer modules.
- Run three tasks: `ui_task` (heartbeat only), `display_task` (color bar),
  `input_task` (5 ms touch polling).
- Establish the console path.
- Keep `default-members = ["crates/grok-ball"]` so host tests do not build the
  firmware.

### Entry gate

- `grok-ball` host tests pass.
- The archived drivers are present and the pin map matches `docs/MOSAICO_V12_HARDWARE.md`.

### Exit gate (`P-R0`)

- Target build passes:
  `cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf`.
- On the device: the console prints the banner and all three heartbeats for at
  least 60 seconds without a watchdog reset.
- The display task transfers a fixed three-band color bar and logs its transfer
  time.
- A touch produces a log line with the first point; release produces a second.
- No LVGL initialization is required for R0.

### Stop conditions

- `esp-rtos` cannot compile for `esp32s31`; stop and evaluate the Embassy-only
  fallback.
- The console path cannot be established; stop before R1. Debugging R1 without
  logs is not allowed.
- The color bar transfer corrupts the panel or resets the board; stop and fix the
  panel initialization before adding LVGL.

## 5. R1 async display and input

### Scope

- Replace the blocking QSPI write with `SpiDma<Async>`.
- Implement two presentation buffers and the `FrameReady`/`FreeBuffer` handshake.
- Move LVGL to `ui_task` with a 10 ms timer.
- Replace touch polling with GPIO6 interrupt + async I2C, keeping the 5 ms
  fallback.
- Transfer dirty rectangles instead of full frames.
- Add P95 instrumentation.

### Entry gate

- `P-R0` is complete.
- The console path is stable.
- The target build includes LVGL and the `lvgl-ui` layer.

### Exit gate (`P-R1`)

- On the device, a touch-to-photon measurement over at least 100 touches gives
  **P95 ≤ 50 ms** and no render/transfer serialization.
- The display never tears during a 60-second swipe/animation run (or the accepted
  tearing policy is recorded if TE is unavailable).
- Queue overflow counters stay at zero during the run.
- The console reports flush count, transfer time, queue depth and P95.

### Stop conditions

- P95 stays above 50 ms after dirty-rectangle transfer and buffer tuning; stop
  UI expansion and diagnose render versus present.
- PSRAM bandwidth prevents 20 fps for the 320×320 ball; reduce the canvas or
  move the draw buffers before adding effects.
- The async SPI DMA API cannot be made sound on S31; stop and evaluate a
  dedicated DMA channel or the `esp_lcd` C adapter.

## 6. R1.5 sensor task

### Scope

- Add `sensor_task` with BMI270, BMM150 and the right-slot Interaction module.
- Share I2C0 with the touch controller through an async mutex.
- Publish `SensorEvent` values through a bounded queue.
- Implement the 250 ms module scan and three-sample debounce.
- Add a stats line for sensor rate, queue depth and dropped events.

### Entry gate

- `P-R0` is complete.
- The module hardware is present in the right slot.

### Exit gate (`P-R1.5`)

- 60-second run with no touch starvation and no I2C bus lockup.
- Sensor event rate and queue depth are reported; dropped edge events are zero.
- Module insertion and removal are detected within 1 second and logged.
- The affect engine consumes the stream on the host replay and on the device.

### Stop conditions

- The module is absent or in the left slot; disable the module inputs gracefully
  and record the hardware state.
- I2C contention starves touch reads; move the sensor to a lower rate or a
  separate bus before continuing.

## 7. Operational entry points

| Gate | Invocation | Purpose |
| --- | --- | --- |
| `P-BASE` | `git status --short; git rev-parse HEAD` in both repositories | Recheck before R0 |
| `P-R0-BUILD` | `cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf` | Target build |
| `P-R0` | Flash `main_app` at `0x210000`; read the console; observe the color bar | R0 device gate |
| `P-R1-BUILD` | Same target build with LVGL and the async path | R1 build |
| `P-R1` | 100 touches with the GPIO/scope instrumentation | P95 ≤ 50 ms |
| `P-R1.5` | 60-second sensor run plus module insertion/removal | Sensor gate |
| `P-DOC` | Link/fence/provenance check | Documentation gate |

### Flash path

The official v0.5.0 partition layout is:

| Region | Offset |
| --- | --- |
| Partition table | `0x8000` |
| `vibe_mode` | `0x20000` |
| `main_app` | `0x210000` |
| `ui_apps` | `0x9E0000` |

Build and flash only the application:

```bash
cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
espflash save-image --chip esp32s31 <ELF> <BIN>
esptool --chip esp32s31 --port <PORT> --baud 115200 \
  --before no-reset --after hard-reset write_flash 0x210000 <BIN>
```

Do not rewrite the bootloader, the partition table or `vibe_mode`. The archived
prototype and the official firmware share this layout; overwriting `0x20000`
breaks the official UI.

## 8. Testing and acceptance

### 8.1 Host tests

- Scheduler-neutral modules: input queue, dirty-rectangle union, buffer
  handshake state machine, P95 accumulator.
- `cargo test -p mosaico-s31 --lib` on the host for those modules.
- The driver modules are `cfg(target_os = "none")` and are covered by the target
  build, not by host tests.

### 8.2 Device acceptance

- R0: console + three heartbeats + color bar.
- R1: 100-touch P95 ≤ 50 ms; no tearing or an accepted policy; queue depth stable.
- R1.5: 60-second sensor stream; module insert/remove within 1 second.
- Record the exact board, power source, panel brightness and firmware revision.

### 8.3 Measurement method

P95 is measured with a GPIO toggle on the touch interrupt and a second toggle at
the first QSPI pixel of the resulting frame, captured by a logic analyzer or
scope. If no logic analyzer is available, use a 240 fps camera and count frames;
record the method and its resolution. Host counters are not device evidence.

### 8.4 Stop conditions

- Any gate fails twice for the same reason; stop and revise the plan.
- A watchdog reset occurs during a run; treat the run as failed.
- The panel shows corruption; stop before measuring latency.

## 9. Milestones and estimates

| Wave | Work package | Deliverable | Exit gate | Estimate |
| --- | --- | --- | --- | --- |
| R0.1 | Crate and workspace | `crates/mosaico-s31`, dependencies, target build | `P-R0-BUILD` | 1–2 days |
| R0.2 | Task skeleton | `esp-rtos` + Embassy, three tasks, console | `P-R0` (console) | 2–3 days |
| R0.3 | Driver port and color bar | Archived drivers, display task color bar | `P-R0` (display) | 1–2 days |
| R1.1 | Async QSPI and buffers | `SpiDma<Async>`, double buffer, `FrameReady`/`FreeBuffer` | `P-R1-BUILD` | 2–3 days |
| R1.2 | Input task | GPIO6 IRQ, async I2C, 5 ms fallback | Touch log on device | 1–2 days |
| R1.3 | Dirty rectangle and P95 | Dirty transfer, instrumentation, 100-touch run | `P-R1` | 2–3 days |
| R1.5.1 | Sensor task | BMI270/BMM150/module, bounded queue | `P-R1.5` | 3–5 days |

R0: **4–6 days**. R1: **5–8 days**. R1.5: **3–5 days**. Total **12–19 days** for
the platform track, excluding hardware repair and official-firmware changes.

## 10. Risks and open decisions

| Risk | Impact | Mitigation |
| --- | --- | --- |
| `esp-rtos` S31 support is incomplete | No task model | Pin the version, inspect the crate source, fall back to an Embassy executor |
| Async SPI DMA API changes | R1 blocked | Pin the esp-hal rev; keep the blocking driver as a fallback |
| PSRAM bandwidth | Frame budget missed | Dirty rectangles; partial internal draw buffers; 240×240 canvas fallback |
| TE not routed or unreliable | Tearing | Timer-based presentation and an accepted tearing policy |
| Console unavailable | No device diagnostics | Establish USB-OTG HS CDC-ACM or UART0 in R0 before R1 |
| I2C contention | Touch starvation | Async mutex, touch priority, sensor rate reduction |
| Partition/flash mistake | Official UI broken | Write only `0x210000`; never rewrite the bootloader or `vibe_mode` |
| Task priority inversion | Latency spikes | Keep display/input high, UI medium, sensors low; measure queue depth |

### Open decisions

| ID | State | Decision | Stop condition |
| --- | --- | --- | --- |
| D-PLAT-01 | `UNKNOWN` | `esp-rtos 0.4.0` + Embassy; fallback to Embassy-only | esp-rtos cannot build for `esp32s31` |
| D-PLAT-02 | `UNKNOWN` | R1a full-frame PSRAM double buffer; R1b partial internal draw buffers | PSRAM render bandwidth misses the frame budget |
| D-PLAT-03 | `UNKNOWN` | TE sync if the signal is routed; otherwise a timer | Tearing exceeds the accepted policy |
| D-PLAT-04 | `CONFIRMED` | USB-OTG HS CDC-ACM console on `USB_HS`; UART0 fallback | Neither path produces logs |
| D-PLAT-05 | `UNKNOWN` | Separate `sensor_task`; merge into `input_task` only if RAM/task count requires | I2C contention or task overhead |
| D-PLAT-06 | `CONFIRMED` | R1.5 may run after R1 or in parallel with it, but not before R0 | R0 console or I2C bus is unstable |

## 11. Handoff

### 11.1 To RFC 0003

`ui_task` calls `Ui::tick(elapsed_ms)` at the selected cadence and
`Ui::timer_handler` for LVGL refresh. The display task transfers the canvas
damage through the dirty-rectangle path. The ball renderer stays on `ui_task`.

### 11.2 To RFC 0004

`sensor_task` publishes `SensorEvent` values. `ui_task` runs `AffectEngine` and
selects `BallExpression`. Haptics run in a non-blocking scheduler or a dedicated
low-priority task; the UI task never sleeps for a pattern.

### 11.3 To C3 and voice

C3, PC media and voice events enter through the same bounded event contract.
They do not call LVGL directly. The UI task remains the only LVGL owner.

### 11.4 Definition of done

- `P-R0`, `P-R1` and `P-R1.5` pass with recorded evidence.
- The console path, buffer mode, TE policy and task priorities are recorded in
  the ledger.
- P95 ≤ 50 ms over 100 touches.
- No watchdog reset, queue overflow or panel corruption during the acceptance
  runs.
- The official `main_app` region is the only flash region written.
