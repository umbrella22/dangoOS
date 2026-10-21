# RFC 0003: Grok-Ball First Screen — Rust/LVGL Engine Port and S31 Integration (Not Implemented)

> Status: **Draft implementation plan; no implementation work package is complete.**
> Plan ID: `mosaico-grok-ball-v1`; revision: `1`.
> Baseline date: `2026-10-20` UTC.
> Baselines: `lv_binding_rust` at `f8b13a48ab027bc4e2be6b884a323407336ed792` plus the local `LV_FONT_MONTSERRAT_48=1` configuration change; `esp-32` at `684e150`; the validated `crates/mosaico-s31` prototype is archived at `archive/crates-pre-grok-ball-20261020/mosaico-s31`.
> Related plans: [RFC 0002 Mosaico UI Layer](../../plans/mosaico-ui-layer-plan.md) (implemented), [RFC 0004 Grok-Ball Sensor Reactions](../../plans/grok-ball-sensor-reactions-plan.md), [RFC 0001 Configurable Rust Component Library](../../plans/component-library-development-plan.md), [Execution order](../../plans/ROADMAP.md).
> External dependency: the RTOS pipeline track R0/R1 described in section 3; it will be specified by RFC 0005 (Platform Runtime) and is not yet written.
> Implementation authorization: **not granted by authoring this RFC.**

Navigation: [Purpose](#1-purpose-and-runtime-rule) · [Baseline](#2-baseline) · [Pipeline dependency](#3-rtos-pipeline-dependency) · [Runtime flow](#4-runtime-flow) · [Phases](#5-phase-boundaries) · [Entry points](#6-operational-entry-points) · [Responsibilities](#7-component-responsibilities) · [Data and engine](#8-data-and-engine-contract) · [Widget contract](#9-custom-widget-contract) · [Acceptance](#10-testing-and-acceptance) · [Milestones](#11-milestones-and-estimates) · [Risks](#12-risks-and-open-decisions) · [Handoff](#13-handoff)

## 1. Purpose and runtime rule

RFC 0003 defines the first screen of the redesigned Mosaico S31 UI: the MIT-licensed `grok-ball` expression engine rendered by a Rust software rasterizer into an LVGL canvas. The current Slint-style Home/Apps screen remains useful as a reference and as the later hub page; it is not the first screen in the new direction.

The ball is one custom widget in the RFC 0002 `lvgl-ui` tree. The application state selects an `AiState`; the ball engine maintains the expression, gaze, blink and transition state; the widget renders a frame into a 320×320 RGB565 canvas. LVGL handles layout, display refresh and touch input. The S31 display pipeline handles panel transfer. RFC 0004 defines the sensor-driven reaction layer that selects additional expressions from the Interaction module, IMU, touch, keys and BMM150; RFC 0003 covers the six AI states and the rendering engine.

The primary runtime rule:

**The UI task has exclusive access to LVGL. `Ui::tick` advances custom-widget animation on that task. The ball renderer writes only its canvas buffer. Display transfer runs on the platform's presentation path.**

The ball never calls LVGL from an interrupt, a network task, the C3 link, or a voice callback. Those inputs enter through `Msg` values and are consumed by `App::update`.

### Goals

- Port the grok-ball data and core math to `no_std` Rust with no JavaScript runtime.
- Support six AI states: `Idle`, `Listening`, `Thinking`, `Speaking`, `Happy`, `Error`.
- Render a 320×320 RGB565 ball at 20–30 fps on S31 after the RTOS pipeline lands.
- Add a custom-widget contract and `Ui::tick` to `lvgl-ui` without changing the existing `Node`/`Msg`/`App` model.
- Keep the ball engine independent from LVGL and `esp-hal`, so host rendering, screenshot tests and parity checks run without hardware.
- Measure frame time, dirty area, memory and touch-to-photon latency after integration.

### Non-goals

- The full 32-expression catalog tuning, ribbon spins, confetti, zzz effects, or exact browser SVG output.
- ThorVG, NanoVG, LVGL vector graphics, PPA/DMA2D, or a GPU renderer in the first cut.
- Voice capture, ASR, LLM calls, C3 protocol, Mi Home, BLE, Lua applications, or Home Assistant.
- Replacing the RFC 0002 reconciler or changing the `Node`/`Msg`/`App` semantics.
- A full-screen animated scene behind the ball. The first screen is a black field with the ball and a minimal status line.
- Pixel-exact browser parity. The acceptance target is silhouette, expression identity, animation feel and measured frame cost.

## 2. Baseline

### 2.1 Current code

| Item | Baseline |
| --- | --- |
| UI layer | RFC 0002 `lvgl-ui` is implemented; host tests, RISC-V target check, gallery and `mosaico-s31` smoke pass |
| Hardware screen | The archived `crates/mosaico-s31/src/ui.rs` rendered the Slint-style Home/Apps screen; display and touch worked; horizontal swipe switching worked, while page-dot hit behavior remains unconfirmed. The crate now lives at `archive/crates-pre-grok-ball-20261020/mosaico-s31` and is not a workspace member. |
| Display path | The archived prototype used a single-threaded pump in `crates/mosaico-s31/src/main.rs`; blocking QSPI `write_chunk`; one retained framebuffer and a 48-line LVGL draw buffer. R0/R1 replace this path. |
| Touch path | 5 ms CST9220 polling; `PointerPort` delivers samples; no GPIO6 interrupt task |
| Console | The application does not expose a USB CDC serial port on the tested board; the bootloader/official app is the only visible USB serial path |
| `lvgl-ui` custom support | `Node::raw` creates an opaque native subtree; there is no custom-widget update hook and no `Ui::tick` |
| LVGL configuration | LVGL 9.6; `LV_USE_CANVAS=1`; `LV_USE_VECTOR_GRAPHIC=0`; `LV_USE_NANOVG=0`; `LV_USE_SNAPSHOT=1`; Montserrat 48 enabled in the local configuration |

### 2.2 Grok-ball source

The reference engine is the MIT-licensed `tycoding/grok-ball` project. The inline browser build contains:

- `EB_RINGS.HEAD_C = 114.2705`, `EYE_HALF = 21`, `STAR_GOLD = "#f4c34e"`.
- `EB_RINGS.EXPRESSIONS`: 25 eye-ring sets, each with a left and right closed outline of 48 points.
- `EB_RINGS.SHAPES.blob.ring`: the body outline.
- `EMOTION_SEED`: 32 expression descriptors with `pool`, `body`, `eyes`, `anims`, `sequence`, `transition`, `blinkMs` and related fields.
- SVG rendering with a `viewBox` of `-15 -15 259 259`, a radial body gradient, dark eye paths, and browser-driven `requestAnimationFrame`.

The browser engine builds SVG DOM nodes. RFC 0003 ports the data and the geometric math, then rasterizes the resulting polygons directly into RGB565.

### 2.3 Six-state map

| `AiState` | Seed | Source name | Role |
| --- | --- | --- | --- |
| `Idle` | `02` | 待机放空 | Default home state; gaze drift and occasional blink |
| `Listening` | `35` | 等待输入 | Voice session armed, waiting for speech |
| `Thinking` | `30` | 思考中 | Request sent, waiting for an answer |
| `Speaking` | `39` | 输出回复 | Answer playback or token stream |
| `Happy` | `10` | 开心 | Successful completion and positive feedback |
| `Error` | `34` | 出错 | ASR, network, C3 or LLM failure |

`Sleep` (`00`) is a later idle-timeout state. The catalog has 32 seeds, so 26 remain outside the six AI states. RFC 0004 makes 14 reachable; the rest stay data-only until G3.

## 3. RTOS pipeline dependency

The ball screen does not fix the current single-threaded display path. The R0/R1 pipeline remains the prerequisite for device acceptance.

| Track | Scope | Gate |
| --- | --- | --- |
| R0 | `esp-rtos` + Embassy scheduler; `ui_task`, `display_task`, `input_task`; a working console | Three tasks run without deadlock; display task can transfer a color bar |
| R1 | `SpiDma<Async>`, double-buffered presentation, `FrameReady`/`FreeBuffer`, GPIO6 + async I2C input task | Touch-to-photon P95 ≤ 50 ms; no render/transfer serialization |

G0 and G1 run on the host and do not wait for R0/R1. G2 integrates the ball into the device only after R1 passes its gate. This order prevents a visually complete ball from hiding a pipeline that cannot meet the interaction target.

The console gap is part of R0. The current application does not enumerate USB CDC on the tested board; R0 must establish a UART0 or USB-OTG CDC log path before device debugging.

## 4. Runtime flow

```mermaid
flowchart TD
    A[Voice/C3/app event] --> B[Msg]
    B --> C[App::update]
    C --> D[AiState and ball parameters]
    D --> E[Ui::tick elapsed_ms]
    E --> F[BallWidget::update]
    F --> G[BallEngine: expression, gaze, blink, body]
    G --> H[Software rasterizer 320x320]
    H --> I[RGB565 canvas buffer]
    I --> J[lv_obj_invalidate(canvas)]
    J --> K[ui_task lv_timer_handler]
    K --> L[display flush and damage]
    L --> M[display_task async QSPI]
    M --> N[CO5300 panel]
```

The ball frame loop runs at 30 fps when an animation is active. Idle states may settle to a lower update rate after the blink/gaze sequence completes. A state change invalidates the canvas immediately; a settled frame does not redraw.

### 4.1 Frame budget

| Item | Target | Notes |
| --- | --- | --- |
| Ball canvas | 320×320 RGB565 | 204,800-byte buffer; PSRAM placement under test |
| Engine + raster | ≤ 20 ms per active frame | Host measurement first; S31 measurement in G2 |
| Active update rate | 30 fps | 33 ms cadence; skip if the previous frame is still pending |
| Dirty upload | 320×320 region | 205 KB at 40 MHz QSPI is about 10 ms; the async presenter overlaps it with rendering |
| Settled idle | 0–10 fps | Stop redraw after the state settles; resume on gaze/blink/voice events |

The 320px size leaves a 80px margin on each side of a 480×480 screen. The ball can grow to 360px only after the frame budget passes.

## 5. Phase boundaries

| Phase | Scope | State crossing the boundary | Exit condition |
| --- | --- | --- | --- |
| G0 | Data extraction, engine math, host rasterizer | JS data -> generated Rust tables; `BallFrame` -> RGB565 buffer | Six states render on host; parity and frame-time tests pass |
| G1 | `lvgl-ui` custom widget + `Ui::tick`; host ball screen | `BallWidget` -> `Node::custom`; `AiState` -> `Msg` | Host `mosaico-s31` preview shows the ball and transitions |
| G2 | S31 integration after R1 | Canvas buffer -> LVGL canvas -> display damage -> panel | Device shows six states; P95 and frame budget pass |
| G3 | Full expression catalog and decorative effects | Extended `ExpressionSeed` data -> engine | Optional; only after G2 acceptance |

G0 and G1 can run in parallel with R0/R1. G2 does not start before R1. G3 does not start before G2.

## 6. Operational entry points

| Gate | Invocation | Purpose |
| --- | --- | --- |
| G-BALL-DATA | `cargo test -p grok-ball --test data_parity` | Generated ring/expression tables match the pinned JS source |
| G-BALL-ENGINE | `cargo test -p grok-ball --test engine` | State transitions, blink, gaze and body math |
| G-BALL-RASTER | `cargo test -p grok-ball --test raster` | Frame hashes, dirty rectangles, allocation count |
| G-BALL-HOST | `cargo run -p grok-ball --example states` | Writes six 320×320 PPM frames for visual review |
| G-UI-CUSTOM | `cargo test -p lvgl-ui --test custom_widget` | Create/update/delete, `Ui::tick`, reentrancy and canvas invalidation |
| G-S31-HOST | `cargo run -p mosaico-s31 --example preview` | Host preview of the ball screen |
| G-S31-TARGET | `cargo check -p mosaico-s31 --target riscv32imafc-unknown-none-elf` | Target compilation |
| G-S31-DEVICE | `esptool --chip esp32s31 ... write_flash 0x210000 ...` plus visual test | Device acceptance after R1 |
| G-BALL-PERF | Device counters and logic-analyzer captures | Frame time, dirty uploads, P95 |

`grok-ball` is a new host-testable crate. It has no LVGL, `esp-hal` or RTOS dependency. The generator lives in `scripts/` or `tools/` and records the source commit, license and SHA-256 of the extracted data.

## 7. Component responsibilities

| Component | Responsibilities | Must not do |
| --- | --- | --- |
| `grok-ball` data module | Store ring sets, body ring, six expression descriptors and color stops | Read files or JavaScript at runtime |
| `BallEngine` | Select expression, interpolate rings, advance gaze/blink/body, expose a frame description | Allocate per frame, call LVGL, block |
| `Rasterizer` | Fill body and eye polygons, apply the body gradient, write RGB565 and compute the dirty rectangle | Touch the display or the app state |
| `BallWidget` | Hold the engine and canvas buffer; create/update/delete the LVGL canvas; invalidate on change | Reconcile children or access other app widgets |
| `lvgl-ui::custom` | Define the custom-widget lifecycle and `Ui::tick` dispatch | Contain ball-specific code |
| `MosaicoApp` | Map `AiState` and future voice/LLM events to `Msg`; place the ball on the screen | Call the engine or LVGL directly |
| `ui_task` (R1) | Call `Ui::tick(elapsed_ms)` at the selected cadence and `Ui::timer_handler` | Run on another thread or call the ball engine directly |
| `display_task` (R1) | Transfer the damaged canvas region and acknowledge buffers | Render or mutate the canvas |

## 8. Data and engine contract

### 8.1 Generated data

The generator evaluates the pinned `grok-ball.js` data objects in a Node.js sandbox and emits:

```rust
pub struct Point { pub x: i16, pub y: i16 }

pub struct EyeRings {
    pub left: [Point; 48],
    pub right: [Point; 48],
}

pub struct ExpressionSeed {
    pub id: u8,
    pub pool: &'static [u8],
    pub transition_ms: u16,
    pub blink_min_ms: u16,
    pub blink_max_ms: u16,
    pub body: BodyParams,
    pub eyes: EyeParams,
    pub anims: &'static [Anim],
    pub sequence: Option<&'static Sequence>,
}
```

The first release emits the 25 ring sets, the body ring, and six selected seeds. `Point` uses `i16` in the engine's 259-unit coordinate space. The data file records the upstream commit, MIT license and SHA-256.

### 8.2 Coordinate space

- Source SVG viewBox: `-15 -15 259 259`.
- Canvas: 320×320.
- Scale: `320 / 259`.
- Translation: `+15` in source units, then scale; or map the source center `114.2705` to the canvas center.
- The engine keeps source-space math for parity; only the rasterizer applies the canvas transform.

### 8.3 Engine behavior

The first release ports:

- Expression selection and pool rotation.
- Interpolation between two ring sets with the seed's transition timing.
- Eye `open`, `scaleX`, `scaleY`, `x`, `y`, `lookX`, `lookY`.
- Blink spring and random blink interval.
- Body `y`, `scale`, `rotate`, `breathe` and body color.
- Idle gaze drift and a settled state.
- Six state transitions with easing.

The first release does not port ribbons, confetti, zzz, sketch jitter, sound-reactive values, or sequence chaining beyond a simple settle.

### 8.4 Rasterizer

The rasterizer writes into a caller-owned `&mut [u16]` of 320×320 pixels.

- Body: polygon fill over `SHAPES.blob.ring`, radial gradient centered near 38%/32%, color stops from the engine.
- Eyes: polygon fill for each transformed eye ring, dark fill.
- Coverage: 2×2 supersampling for the body and eye edges; no per-pixel `lv_canvas_set_px` calls.
- Clip: a fixed 320×320 rectangle; no allocation after initialization.
- Dirty rectangle: union of the previous and current ball bounds; the first release may invalidate the full canvas, with per-region invalidation deferred to G2 tuning.
- Frame buffer: the `BallWidget` retains one buffer; the rasterizer writes it in place; LVGL reads it through `lv_canvas_set_buffer`.

### 8.5 Canvas integration

`BallWidget::create` creates an `lv_canvas`, sets the 320×320 RGB565 buffer and centers it. `BallWidget::update` runs the engine for `elapsed_ms`, rasterizes when a new frame is due, and calls `lv_obj_invalidate(canvas)`. `BallWidget::delete` releases the canvas and buffer through the mounted-tree lifecycle.

The canvas buffer is a `Box<[u16; 320 * 320]>` allocated once. Placement in PSRAM is the default; internal RAM is measured as an alternative. The buffer address remains stable for the canvas lifetime.

## 9. Custom widget contract

RFC 0002 exposes `Node::raw` for one-time opaque subtrees. The ball requires update and tick hooks. RFC 0003 adds a narrow contract to `lvgl-ui`:

```rust
pub trait CustomWidget<M>: 'static {
    fn create(&mut self, parent: &mut Obj<'_>) -> Result<CustomObject, UiError>;
    fn update(&mut self, object: &mut CustomObject, elapsed_ms: u32) -> Result<(), UiError>;
    fn delete(&mut self, object: &mut CustomObject);
}

impl<M> Node<M> {
    pub fn custom(widget: Box<dyn CustomWidget<M>>) -> Self;
}

impl<M: Clone + 'static> Ui<M> {
    pub fn tick(&mut self, elapsed_ms: u32) -> Result<(), UiError>;
}
```

`CustomObject` is an opaque handle managed by `lvgl-ui`; it carries the native object identity without exposing a Rust lifetime that could outlive the mounted tree. The exact checked native accessor used by the ball canvas is frozen in G1.1.

Rules:

- `create`, `update` and `delete` run on the UI task.
- `Ui::tick` walks mounted nodes once and calls `update` on each custom widget. It does not rebuild the `Node` tree and does not process messages.
- A custom widget may mutate only its native subtree and canvas buffer. It cannot reconcile children or mutate sibling objects.
- `update` returns without invalidating when the frame is unchanged.
- A kind change or key removal deletes the custom subtree through the existing reconciler lifecycle.
- `Ui::tick` is not reentrant with `Ui::drain` or C event dispatch. The platform calls it outside `lv_timer_handler`.
- A custom widget's callbacks follow the same event-queue rule as typed widgets: they post messages and never mutate the tree.

The contract freezes in G1.1 before the ball widget is implemented. The existing `Node::raw` remains for static opaque widgets.

## 10. Testing and acceptance

### 10.1 Host tests

| Layer | Cases | Oracle |
| --- | --- | --- |
| Data parity | 25 ring sets, 6 seeds, body ring | Point coordinates match the JS source within ±0.5 source units |
| Engine | Each of six states; state transition; blink; gaze; settled idle | Frame hashes and state/timing counters |
| Raster | Body-only, eyes-only, six full states; 1,000 frames | Pixel hashes, dirty rectangles, zero steady-state allocations |
| Timing | 320×320 active frames; 240×240 fallback | Host mean/P95 frame time; no target claim from host numbers |
| Custom widget | Create/update/delete; `Ui::tick`; key replacement; no-op frame | Object count, canvas invalidation count, no leaks |
| UI screen | Host preview at 480×480 | Six screenshots; ball centered; black field; no clipping |

### 10.2 Device acceptance after R1

- The six states are selectable through `Msg::AiState` from a temporary touch/voice stub.
- The ball is visible at 320×320 on the 480×480 panel and centered with at least 70px margin.
- Active animation reaches 20–30 fps; the frame budget is recorded, not inferred.
- Touch-to-photon P95 is ≤ 50 ms on the R1 pipeline.
- A settled idle state stops redrawing.
- PSRAM/internal RAM counts, canvas buffer placement and stack high-water are recorded.
- No tearing, no display underrun, no SPI errors.
- The console path from R0 is available during the device test.

### 10.3 Stop conditions

- Rasterizer mean frame time exceeds 33 ms after the 240×240 fallback and reduced update rate; stop and evaluate pre-rendered sprites.
- Data parity exceeds the ±0.5-unit tolerance; stop and fix the extraction or math before visual tuning.
- `Ui::tick` cannot run without violating the single-UI-thread rule; stop and revise the custom contract.
- R1 P95 stays above 50 ms; stop UI expansion and diagnose render versus present.
- Canvas placement or PSRAM bandwidth prevents 20 fps; reduce the canvas or move the buffer before adding effects.

## 11. Milestones and estimates

| Wave | Work package | Deliverable | Exit gate | Estimate |
| --- | --- | --- | --- | --- |
| G0.1 | Data extraction | Generator, generated Rust tables, license/provenance record, parity tests | `G-BALL-DATA` | 2–3 days |
| G0.2 | Engine core | Six seeds, ring interpolation, gaze, blink, body motion, state transitions | `G-BALL-ENGINE` | 3–5 days |
| G0.3 | Rasterizer | Body gradient, eye fills, 320×320 RGB565 output, host screenshots | `G-BALL-RASTER`, `G-BALL-HOST` | 4–6 days |
| G1.1 | Custom widget contract | `CustomWidget`, `Node::custom`, `Ui::tick`, lifecycle tests | `G-UI-CUSTOM` | 3–5 days |
| G1.2 | Ball widget and screen | `BallWidget`, `MosaicoApp` ball screen, host preview | `G-S31-HOST` | 2–3 days |
| G2.1 | S31 integration | Canvas on device, state stub, console, first visual acceptance | `G-S31-TARGET`, `G-S31-DEVICE` | 3–5 days |
| G2.2 | Performance tuning | Dirty region, frame cadence, PSRAM/internal comparison, P95 | `G-BALL-PERF` | 3–5 days |
| G3 | Full catalog | Remaining expressions, ribbons, confetti, zzz, optional effects | Separate amendment | 2–4 weeks |

G0+G1 estimate: **14–22 working days** after authorization. G2 adds **6–10 working days** after R1. The first screen is about **3–4 weeks** of focused work once R0/R1 is available; G0 can start immediately in parallel with R0/R1.

## 12. Risks and open decisions

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Software polygon fill is slower than the frame budget | Ball stutters | 2×2 coverage first; 240×240 fallback; pre-rendered sprites as the final fallback |
| PSRAM canvas bandwidth | Frame writes compete with display reads | Measure internal RAM; reduce canvas; write only dirty rectangles |
| Anti-aliasing cost | CPU budget consumed by edges | Edge coverage only; disable AA in the fallback |
| `lvgl-ui` custom API complexity | Reconciler lifecycle bugs | Freeze the contract in G1.1; use existing `MountedNode` lifecycle and delete callbacks |
| UI tick reentrancy | Object mutation during LVGL dispatch | Call `Ui::tick` outside `lv_timer_handler`; reject reentrant calls |
| Data parity | Wrong expression shapes | Pinned source, generator, ±0.5-unit tests |
| RTOS R1 schedule | No device integration | G0/G1 host work proceeds independently; G2 waits for R1 |
| Console unavailable | No device diagnostics | R0 must establish UART0 or USB-OTG CDC before G2 |
| Full 32-state scope creep | Delay first screen | Six AI states; RFC 0004 exposes 14 sensor states; G3 is a separate amendment |
| Browser SVG differences | Visual mismatch | Accept silhouette/expression parity; compare screenshots, not DOM |
| MIT attribution | License compliance | Keep upstream notice, source commit and data SHA-256 in the generated module and README |

### Open decisions

| ID | State | Decision | Stop condition |
| --- | --- | --- | --- |
| D-BALL-01 | `UNKNOWN` | 320×320 initial canvas; 240×240 fallback | Host/S31 frame budget fails |
| D-BALL-02 | `CONFIRMED` | Software polygon rasterizer in `grok-ball`; no LVGL vector backend | Rasterizer cannot meet 33 ms after fallback |
| D-BALL-03 | `UNKNOWN` | Six seed mapping (`02`,`35`,`30`,`39`,`10`,`34`) | Voice/product semantics require a different seed |
| D-BALL-04 | `UNKNOWN` | Canvas in PSRAM; internal RAM comparison in G2 | PSRAM bandwidth or allocation fails |
| D-BALL-05 | `CONFIRMED` | `CustomWidget` + `Ui::tick` extension to RFC 0002 | Contract cannot preserve single-thread mutation rule |
| D-BALL-06 | `UNKNOWN` | Full-canvas invalidation in G1; dirty rectangle in G2 | Frame budget fails before dirty-region work |
| D-BALL-07 | `CONFIRMED` | R0/R1 precede G2; G0/G1 can start now | R1 P95 remains above 50 ms |

## 13. Handoff

### 13.1 To the RTOS pipeline

G0/G1 produce a host-testable `grok-ball` crate and a `BallWidget` that consume `elapsed_ms` and a `BallState`. R1's `ui_task` calls `Ui::tick(elapsed_ms)` at the selected cadence and `Ui::timer_handler` for display refresh. The display task receives the canvas damage through the existing display damage path; it does not call the ball engine.

### 13.2 To C3 and voice

C3, ASR and the LLM produce `AiState` and optional gaze/audio-level messages. They enter through `Ui::post` or the platform message source. The ball does not read network, audio or C3 state directly. `Listening`, `Thinking`, `Speaking` and `Error` map to the six-state engine; `Happy` is an explicit success event.

### 13.3 Definition of done

- G0 data parity, engine and raster tests pass.
- G1 custom-widget lifecycle and host ball screen pass.
- R1 pipeline gate passes.
- G2 device shows the six states at 20–30 fps with P95 ≤ 50 ms and no tearing.
- The generated data records the upstream commit, MIT notice and SHA-256.
- The plan and execution ledger record every gate, command and hardware observation.
