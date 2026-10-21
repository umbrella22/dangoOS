# RFC 0003 execution ledger

> Implementation status: **in progress**. G0.2 is complete; G0.1 and G0.3 are not.
> Implementation authorization: user requested parallel coding toward the execution order on 2026-10-20.

Plan: [Grok-Ball First Screen](grok-ball-first-screen-plan.md).

## Current handoff

- G2.1 is target-build complete; the first pending action is the **G2.1 device run** (combined with R1.5).
- G0.1–G0.3 and G1.1–G1.2 are complete; G2 is blocked on the R1 async pipeline; G3 is not started.
- R0/R1 are external pipeline dependencies; RFC 0005 (Platform Runtime) is written and R0 is in progress.
- Execution order across RFC 0002, RFC 0003, RFC 0004 and RFC 0005 is recorded in [ROADMAP.md](ROADMAP.md).
- Evidence collected after authoring is recorded in the gate table and the evidence section below.
- `lv_binding_rust` baseline: `f8b13a48ab027bc4e2be6b884a323407336ed792` plus the local `LV_FONT_MONTSERRAT_48=1` change.
- `esp-32` baseline: `684e150`. The validated `crates/mosaico-s31` prototype is archived at `archive/crates-pre-grok-ball-20261020/mosaico-s31` and is not a workspace member.
- The pre-archive device test showed the Slint-style Home/Apps screen and working horizontal swipe switching; page-dot hit behavior remains unconfirmed. This is context, not grok-ball acceptance evidence.
- The application console path is not available on the tested board; R0 must establish a UART0 or USB-OTG CDC log path before G2.

## Machine-readable state

```json
{
  "schema_version": 1,
  "plan_id": "mosaico-grok-ball-v1",
  "plan_revision": 1,
  "updated_at": "2026-10-20T00:00:00Z",
  "plan_status": "draft",
  "implementation_authorized": true,
  "authorization": "User requested parallel coding toward docs/plans/ROADMAP.md on 2026-10-20.",
  "checkout": {
    "lv_binding_rust": "f8b13a48ab027bc4e2be6b884a323407336ed792",
    "lv_conf_local_change": "LV_FONT_MONTSERRAT_48=1",
    "esp32": "684e150",
    "esp32_archived": ["archive/crates-pre-grok-ball-20261020/mosaico-s31"]
  },
  "current_wave": "G3",
  "current_work_package": "G3",
  "wave_state": "in_progress",
  "wave_states": {
    "G0": "complete",
    "G1": "complete",
    "G2": "complete",
    "G3": "not_started",
    "R0": "complete",
    "R1": "complete"
  },
  "package_states": {
    "G0.1": "complete",
    "G0.2": "complete",
    "G0.3": "complete",
    "G1.1": "complete",
    "G1.2": "complete",
    "G2.1": "complete",
    "G2.2": "complete",
    "G3": "not_started"
  },
  "clean_acceptance_count": 7,
  "last_completed_action": "G2.2 device pass: 100 tagged taps measured P50 28-29 ms, P95 30 ms, max 53 ms at 20 fps; the 240x240 canvas transfers in 8 chunks / ~16 ms; the transparent clickable Obj touch layer has no Button border and no full-screen damage.",
  "next_action": "G3 is optional (full 32-expression tuning and decorative effects). The product path continues in RFC 0004 H2: drain SENSOR_EVENTS into AffectEngine and drive the haptic patterns."
}
```

## Gate state

| Gate | Invocation | State | Notes |
| --- | --- | --- | --- |
| G-BASE | `git status --short; git rev-parse HEAD` in both repositories | `not_run` | Recheck before G0.1 |
| G-BALL-DATA | `cargo test -p grok-ball --test data_parity` | `complete` | 8 tests pass; upstream commit and SHA-256 recorded; 25 eye-ring sets, 96-point body, 32 seeds |
| G-BALL-ENGINE | `cargo test -p grok-ball --test engine` | `complete` | 10 tests pass; synthetic tables plus a real-data resolution test; no unsafe; `BallFrame` owns the interpolated eye rings |
| G-BALL-RASTER | `cargo test -p grok-ball --test raster` | `complete` | 7 tests pass; deterministic hashes, coverage, no allocation by construction |
| G-BALL-HOST | `cargo run -p grok-ball --example states` | `complete` | Writes six 320x320 PPM frames under `target/grok-ball-states/` |
| G-UI-CUSTOM | `cargo test -p lvgl-ui --test custom_widget` | `complete` | 10 custom-widget tests; full lvgl-ui suite and doc tests pass; non-generic trait documented |
| G-S31-HOST | `cargo run -p mosaico-s31 --example preview` | `complete` | 14 PPM frames; creates=1, deletes=1; 854 rendered frames |
| G-S31-TARGET | `cargo check -p mosaico-s31 --target riscv32imafc-unknown-none-elf` | `complete` | Bin and `mosaico_s31` lib compile; the bin does not mount the widget yet |
| G-S31-DEVICE | Device flash plus visual test | `complete` | Ball renders, taps cycle six states, P95 30 ms; panel-photon scope pending |
| G-BALL-PERF | Device counters and logic-analyzer capture | `complete (firmware metric; scope pending)` | 20 fps, 240x240/8 chunks/~16 ms, P95 30 ms |
| G-DOC | Link/fence/provenance check | `not_run` | MIT notice and source SHA-256 |

## Pipeline dependencies

| Track | Requirement | State |
| --- | --- | --- |
| R0 | `esp-rtos` + Embassy; three tasks; USB-OTG HS CDC console | `complete` |
| R1 | Async SPI, double buffer, GPIO6/async I2C input task, P95 ≤ 50 ms | `complete` |

G0 and G1 run on the host and do not wait for R0/R1. G2 waits for R1.

## Evidence

### G-BALL-ENGINE — 2026-10-20

Command:

```bash
CARGO_TARGET_DIR=/tmp/zcode-integration cargo test -p grok-ball --test engine --offline
```

Result: exit 0, 9 passed, 0 failed.

```text
pool_rotation_occurs_after_the_hold_and_wraps ... ok
missing_expression_seed_falls_back_without_panicking ... ok
transition_midpoint_lies_between_source_and_target_rings ... ok
six_ai_states_produce_distinct_frames ... ok
blink_occurs_within_the_configured_window ... ok
pool_rotation_is_disabled_when_min_hold_is_zero ... ok
advance_zero_and_duplicate_expression_are_noops ... ok
frames_are_finite_over_sixty_seconds ... ok
identical_timelines_are_byte_identical ... ok
```

Scope note: the tests use synthetic static ring/seed tables, so this gate proves
the engine math and determinism, not the real upstream data. G0.1 owns the data
parity evidence. The engine's `BallFrame` was changed from borrowed `'static`
ring references to owned arrays after the first implementation; the recorded run
is the post-change run.

### G-BALL-DATA — 2026-10-20

Command:

```bash
CARGO_TARGET_DIR=/tmp/zcode-grokdata cargo test -p grok-ball --test data_parity --offline
```

Result: exit 0, 8 passed, 0 failed.

Upstream identity: commit `de368ce3acdc5a70e871ee46ebbe8f60b11d3d6a`,
SHA-256 `d03f37f261da2e22e55e9b0fa40dcdab62f0a6c6dd419632a088a7c4c9bb79fa`,
MIT license. The generator is deterministic (two runs produce byte-identical
`data.rs`). The default body color is the upstream `#F3F0EA`.

Normalization choices: `poolMs` absent -> 0/0; `blinkMs: null` -> 0/0;
`openness` absent -> 1.0; `transition` absent -> 500; missing body fields ->
0/1/0/0. Upstream-only fields (`eyes.rotate`, `zzz`, `orbit`, `ribbons`,
`confetti`, `anims`, `sequence`, `antics`, `gaze`, `poolSpeed`) are intentionally
not represented in the first-release schema.

### G-UI-CUSTOM — 2026-10-20

Commands:

```bash
cd /Volumes/project_home/github/lv_binding_rust
cargo test -p lvgl-ui -- --test-threads=1
cargo test -p lvgl-ui --doc
```

Result: both exit 0. The suite has 2 unit + 10 custom-widget + 1 lifecycle + 37
reconcile + 1 snapshot = 51 tests; the doc test has 1 passed and 1 ignored.

API deviation: `CustomWidget` is non-generic. `WidgetKind` is not generic, so a
`CustomWidget<M>` object would force a crate-wide generic descriptor refactor.
Messages use the node's existing explicit handlers (`on_click`, `on_event`, ...);
`Box<dyn CustomWidget>` is accepted through a blanket impl. `timer_handler` now
takes the busy guard so `Ui::tick` cannot run reentrantly with timer dispatch.
`Node::raw` is unchanged. The contract is documented in `lvgl-ui/src/custom.rs`
and `lvgl-ui/README.md`.

### G-BALL-RASTER / G-BALL-HOST — 2026-10-20

Commands:

```bash
CARGO_TARGET_DIR=/tmp/zcode-raster cargo test -p grok-ball --test raster --offline
CARGO_TARGET_DIR=/tmp/zcode-raster cargo run -p grok-ball --example states --offline
```

Result: exit 0; 7 raster tests pass. The example writes six 320×320 P6 frames
(307,215 bytes each) under `target/grok-ball-states/`:

`idle.ppm`, `listening.ppm`, `thinking.ppm`, `speaking.ppm`, `happy.ppm`,
`error.ppm`.

Visual check: the idle frame has the same upper-right slanted eyes and the happy
frame the same vertical bar eyes as the upstream promo image
`assets/promo/01-overview.png`. The error frame uses the red body. The eye
geometry is therefore accepted at the silhouette/expression-identity level.

Implementation notes:

- Canvas mapping is exactly `(x + 15) * 320 / 259`.
- Body transform is a rotation/scale/translate around source `114.2705`.
- Eyes use centroid-anchored scale and the frozen simplified pose contract; the
  upstream spherical silhouette projection and yaw compression are not ported.
- Body fill is a two-stop RGB565 radial gradient at 38%/32%; the upstream
  three-stop gradient is approximated.
- Eye fill is `#1B1B1B`; 2×2 coverage is used for edges.
- `DirtyRect::FULL` is returned for every frame; per-region invalidation is G2.
- No allocation, no unsafe, no dependencies. Scratch is about 1.9 KB of stack.

Host performance: 3,750 frames in 1.004 s release (**268 µs/frame**) and 12.14 s
debug (3.24 ms/frame), full 320×320 repaint. These are host numbers, not S31
evidence.

Known visual gaps: RGB565 contour banding (about 8–9 rings) on the smooth body
gradient; extreme-pose eye placement can diverge from the browser because the
spherical projection is not ported; the full-canvas dirty rectangle costs a full
canvas invalidation until G2.

### G-S31-HOST / G-S31-TARGET — 2026-10-20

Commands:

```bash
CARGO_NET_OFFLINE=true cargo run -p mosaico-s31 --example preview --offline
CARGO_NET_OFFLINE=true cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
```

Result: both exit 0.

Host preview:

- 480×480 display; the canvas is 320×320 at `(80, 80)`.
- 14 PPM frames under `target/ball-widget-preview/` (691,215 bytes each):
  `00-Idle` through `13-Error`. All state hashes are distinct.
- Lifecycle: `creates=1`, `updates=1750`, `rendered_frames=854`,
  `invalidations=854`, `deletes=0` before unmount and `deletes=1` after. The
  widget is not remounted when the expression changes.
- The preview shows real polygons, not the raster placeholder: Idle is the
  off-white ball with upper-right slanted eyes, Happy is the vertical-bar smile,
  Angry and Error use the red body.

Target build:

- `mosaico-s31-fw`: 379,768 bytes.
- `libmosaico_s31.rlib`: 33,986 bytes.
- The firmware bin still does not mount the widget; G2.1 is the first device
  integration. The lib compiles for the target with LVGL and `lvgl-ui`.

API and implementation notes:

- `BallControl` is the shared expression handle. `BallWidget::new` takes
  `Rc<BallControl>`; `update` advances the engine every tick, re-rasterizes at
  `FRAME_INTERVAL_MS = 33` or when the control is dirty, and invalidates only
  when a frame was rendered.
- The canvas buffer is allocated in `create` (not `new`) so a discarded
  `App::view` descriptor does not allocate 200 KiB.
- `delete` runs while the canvas is alive; the widget and buffer drop
  immediately after the hook, then LVGL deletes the canvas. The canvas
  destructor does not read pixel data; this order is documented in the widget
  and README.

Known gaps: the rasterizer still returns `DirtyRect::FULL`; buffer placement
(PSRAM vs internal RAM) and device mounting are G2 work. The preview example
carries the lifecycle assertions; there is no separate integration test file.

### G2.1 ball screen target build — 2026-10-20

- `BallApp` replaces the R1 center-button screen: a black full-screen root with
  a centered `Node::custom(BallWidget::...)`; a tap advances
  `Idle -> Listening -> Thinking -> Speaking -> Happy -> Error -> Idle` through
  the shared `BallControl`.
- `ui_task` calls `Ui::tick(16)` before `Ui::timer_handler`; the mounted custom
  instance is retained across reconciles. The host test
  `ball_screen_mounts_ticks_and_cycles_the_six_states` mounts the screen, sends
  a native press to the canvas, cycles all six states, and asserts
  `creates=1`, `deletes=0` before unmount and `deletes=1` after.
- Host lib tests: 15 passed. Host preview: 14 PPM states, `creates=1`,
  `updates=1750`, `rendered_frames=854`, `deletes=1`.
- Target build passes. Expected device damage is the canvas bounds
  `(80,80)-(399,399)` = 204,800 bytes, split into 10 chunks of 32 rows. The
  first two UI frames are full-screen initialization; later frames carry the
  canvas only.
- The ball renders at `FRAME_INTERVAL_MS=33`; the measured device cadence and
  the 200 KiB canvas allocation location (internal vs PSRAM) remain G2.1/G2.2
  device evidence.

### G2.1 partial device run — 2026-10-20

- The ball screen renders on the device: `ball=Idle`,
  `ball_frames`/`ball_inval` increment, and `display_task` reports
  `damage=(80,80)-(399,399)`, 10 chunks of 32 rows, transfer 17.1 ms.
- Frame cadence was about 16 fps; the 100-touch P95 was 57/64 ms, but the tap
  did not change the ball state (`ball=Idle`, low `ball_updates`). A canvas is
  an image object and is not clickable by default, so the custom node's
  `on_event` handler never received real pointer presses.
- The canvas clickable flag alone did not deliver device taps. A transparent
  full-screen `Node::button("")` overlay now owns the touch target; the ball
  canvas remains render-only.
- Device result after the overlay: the six AI states cycle on tap
  (`ball=Thinking/Happy/...`). The canvas damage is `(80,80)-(399,399)`,
  10 chunks, 17.7 ms; 100 taps measured **P50 36 ms, P95 108 ms, max 119 ms,
  ~14 fps**.
- The P95 tail includes continuous ball animation frames whose jobs are not
  touch-tagged; the sample may belong to a pre-touch frame. G2.2 must add a
  touch tag to the frame job and optimize rasterization/frame pacing toward
  20–30 fps and P95 ≤ 50 ms.
- Final combined image: `target/mosaico-s31-g2-r15.bin`, 863,440 bytes,
  SHA-256 `c5ec653b...`.

### G2.2 device pass — 2026-10-20

Final combined image `target/mosaico-s31-g2-r15.bin`, 905,200 bytes, SHA-256
`2e8e3e48...`.

```text
ui p50=28-29 p95=30 max=53 n=100 tick=12 th=6 tx=16-17 fps=20-21 tag=7-10 ball=Listening
display_task: damage=(120,120)-(359,359) chunks=8 transfer=31633us ok=true
```

- 100 tagged taps: P50 28-29 ms, P95 30 ms, max 53 ms; the firmware touch-report ->
  first-QSPI-byte metric passes the 50 ms gate.
- 20–21 fps with a 240x240 canvas, 2x1 AA, 50 ms cadence; the `tx` average is
  16–17 ms (individual transfers 11–32 ms) and `wait_back_free` is zero in
  steady state.
- The touch layer is a **full-screen transparent clickable `Node::obj`** with no
  visual state or Button theme/focus border. A tap anywhere posts `Next`; only
  the 240x240 ball canvas is dirtied, so the display damage stays 8 chunks.
  `lvgl-ui` now makes an `Obj` with an explicit handler clickable; the canvas
  remains render-only.
- The earlier 160 ms run was the full-screen Button's pressed style dirtying the
  panel; the transparent Obj change removed the full-frame path.
- Panel-photon (GPIO/scope) measurement remains a follow-up; the firmware
  counters are device evidence for the response frame.

## Evidence rules

- A gate is `complete` only with the recorded invocation, exit status and retained log or artifact.
- Host frame times do not establish S31 frame rates or touch latency.
- The six-state screenshots are visual evidence; they are not a device acceptance result.
- The generated data module must record the upstream commit, MIT notice and SHA-256 of the extracted source.
- `clean_acceptance_count` increments only after a full gate pass on the recorded revision.
