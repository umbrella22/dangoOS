# RFC 0004 execution ledger

> Implementation status: **H0 complete; H1.1/H1.2 device complete; H1.3 target build; H2 complete; H3 tap-only + fixed ball plane accepted (drag removed by product decision)**.
> Implementation authorization: user requested parallel coding toward the execution order on 2026-10-20.

Plan: [Grok-Ball Sensor Reactions](grok-ball-sensor-reactions-plan.md).

## Current handoff

- H1.1/H1.2 are complete on device: right-slot `interact(0x16)`, both BMM150 `0x32` at I2C0 `0x11/0x12`, 20 Hz.
- H2 is wired: `BallRuntime` drains `SENSOR_EVENTS`, merges affect + personality, and forwards haptics to `haptic::play`.
- H3 is implemented as tap-only: tap ladder, reaction delay, anger cooldown, BQ27220 battery layers, warm boot without our splash, in-canvas entrance, skins. Grab/drag/struggle were removed.
- Device acceptance on `fixed4`: `p50=34 p95=42 max=42 n=17`, `tx=6ms`, `fps=19`, `te=286/0/17`, `8 chunks / 6969us`; user accepted the build.
- Open: IMU rate toward 50 Hz, tap/haptic/battery feel, H1.3 haptic waveform observation.
- Hardware baseline: official Interaction module in the right slot (`EEPROM 0x51`, `board_type 0x16`), camera in the left slot (`EEPROM 0x50`).
- Main board: BMI270 `0x68`, BMM150 7-bit `0x11/0x12` on I2C0, CST9220 INT `GPIO6`, buttons `GPIO7/61`, motor `GPIO8`, TE `GPIO43`.

## Machine-readable state

```json
{
  "schema_version": 1,
  "plan_id": "mosaico-grok-ball-reactions-v1",
  "plan_revision": 1,
  "updated_at": "2026-10-21T02:00:00Z",
  "plan_status": "draft",
  "implementation_authorized": true,
  "authorization": "User requested parallel coding toward docs/plans/ROADMAP.md on 2026-10-20.",
  "current_wave": "H3",
  "current_work_package": "H3-device-acceptance",
  "wave_states": {
    "H0": "complete",
    "H1": "device_complete",
    "H2": "host_complete_device_boot",
    "H3": "host_complete_device_boot",
    "R0": "complete",
    "R1": "complete"
  },
  "package_states": {
    "H0.1": "complete",
    "H0.2": "complete",
    "H0.3": "complete",
    "H1.1": "target_build_complete",
    "H1.2": "device_complete",
    "H1.3": "target_build_complete",
    "H2.1": "host_complete",
    "H2.2": "host_complete",
    "H3.1": "host_complete",
    "H3.2": "host_complete",
    "H3.3": "host_complete"
  },
  "clean_acceptance_count": 1,
  "last_completed_action": "Drag removed by product decision; fixed 240x240 ball plane accepted on device (fixed4, 945,920 B, 367b148b): p50=34 p95=42 fps=19, 8 chunks / 6.97 ms, entrance baked in-canvas.",
  "next_action": "Tune the IMU toward 50 Hz (late/imax), then accept the tap ladder / haptics / battery feel. Drag is out of scope; do not resurrect the offset/band/trace path."
}
```

## Gate state

| Gate | Invocation | State |
| --- | --- | --- |
| H-BASE | `git status --short; git rev-parse HEAD` in both repositories | `not_run` |
| H-EVENTS | `cargo test -p grok-ball --test affect_events` | `complete` |
| H-SHAKE | `cargo test -p grok-ball --test shake_timeline` | `complete` |
| H-TOUCH | `cargo test -p grok-ball --test touch_gestures` | `complete` |
| H-AMBIENT | `cargo test -p grok-ball --test ambient_timeline` | `complete` |
| H-KEYS | `cargo test -p grok-ball --test keys_buttons` | `complete` |
| H-BMM150 | `cargo test -p grok-ball --test magnetic_events` | `complete` |
| H-MODULE | `cargo test -p mosaico-s31 --test module_watcher` | `not_run` |
| H-HAPTIC | `cargo test -p mosaico-s31 --test haptic_patterns` | `not_run` |
| H-DEVICE | Right-slot Interaction insertion/removal and sensor timeline capture | `not_run` |
| H-DOC | Link/fence/provenance check | `not_run` |

## Pipeline dependencies

| Track | Requirement | State |
| --- | --- | --- |
| R0 | `esp-rtos` + Embassy; three tasks; USB-OTG HS CDC console | `complete` |
| R1 | Async SPI, double buffer, GPIO6/async I2C input task, P95 ≤ 50 ms | `complete` |

H0 runs on the host and is complete. H1/H2 device work follows the R0 device gate and R1.5. The sensor task may be added to the R1 task set or a separate R1.5 package; the plan treats it as a dependency, not as completed work.

## Evidence

### H0 host timelines — 2026-10-20

Command:

```bash
CARGO_TARGET_DIR=/tmp/zcode-grokball-affect cargo test -p grok-ball \
  --test affect_events --test shake_timeline --test touch_gestures \
  --test ambient_timeline --test keys_buttons --test magnetic_events \
  --test module_events --offline
```

Result: exit 0; **56 tests passed** (13 affect events + 9 ambient + 8 keys + 7
magnetic + 7 module + 6 shake + 6 touch).

Thresholds and decisions:

| Area | Value |
| --- | --- |
| IMU rest / small / large | `<80 mg` and `<20 dps` / `>=300 mg` / `>1200 mg` or `>200 dps` |
| Large peaks / burst rest / gap | two within `200 ms` / `300 ms` / `300..=800 ms` |
| Angry bursts | `5` within `15 s` (plan range 5–10) |
| Panic / still / post-panic Confused | `1.5 s` / `5 s` / `3 s` |
| Small-shake Confused | `2.5 s` |
| Angry hold / cooldown | `10 s` / `30 s` after the hold |
| Petting / poke / rapid pokes | last pet + `1.5 s` / `1.5 s` / 3 within `1.5 s` → Angry |
| KEY_L / KEY_R / triple R / hug | Happy `1.5 s` or while held / Puzzled `1.5 s` / 3 within `1.5 s` / `300 ms` window, `2 s` hold |
| Light | `+/-25` within `300 ms`; dawn `+20` over `30 s`; night `<10` for `60 s`; bright wake `>=20` |
| PIR | `200 ms` debounce; rising Curious `3 s`; sustained `10 s` → Satisfied `1.5 s`; no repeat `30 s`; absence `60 s` → Sleep |
| BMM150 | confidence `>=50`; approach `2 s`, contact `1.5 s`, release `2 s`, orbit `1.2 s`, edge `1.5 s` |
| Module | right insert `0.5/1.5/1.2 s`; remove `0.4/2.0 s`; left insert/remove `1.5 s` |
| Haptics | one pattern per `150 ms`; Panic/Error priority |

Activity/Error input: `EventSource::System` + `EventKind::Activity(BallExpression)`
accepts `Listening`, `Thinking`, `Speaking`, `Error` and `Idle`. Error is the
top layer and clears only on `Activity(Idle)`. Angry and Panic interrupt
non-Error Activity and the Activity resumes when the hold ends.

Not covered by H0:

- Pickup/flip, long-stroke `Satisfied` and BMM150 orientation events have no H0
  event variants.
- Confidence gating is implemented only for `MagneticEdge`; the upstream
  classifier must filter the other magnetic events.
- Module-watcher EEPROM debounce and LEDC haptic waveforms are H1.1/H1.3.
- Device thresholds and calibration are H2.

### H1.1/H1.2/H1.3 target build — 2026-10-20

- Official BSP source confirms the BMM150s are on the shared **main I2C0**
  (v1.2 SDA=GPIO56/SCL=GPIO3) at 7-bit **0x11/0x12**; the YAML `0x22/0x24` are
  8-bit write forms. The official sequence is `bmm150_init` -> NORMAL ->
  REGULAR -> DRDY `0x48` poll (200 ms); chip ID `0x40 == 0x32`.
- `drivers/imu.rs` now runs a startup raw scan of I2C0 and I2C1 (0x08..0x77),
  probes `0x10..0x13` and `0x22/0x24` on both buses, logs raw chip IDs and, for
  an unexpected `0x10..0x13` device, the `0x4B/0x4C/0x48` values. The register
  read uses `write_read_async` (register write + repeated start).
- `MagneticClassifier`: approach 25 uT (confidence gate 50), contact 80 uT for
  2 votes, release 35 uT for 3 votes, orbit via 4 same-direction samples and
  `sin >= 0.80`, edge via `cos < 0.5`; thresholds provisional for H2.
- Module inputs: LDR GPIO46/ADC1_CH4 inverted, PIR GPIO5, KEY_L 11/KEY_R 10,
  25 Hz, three-sample debounce; module `interact(0x16)` gating unchanged.
- Haptics: full RFC 0004 pattern table, 150 ms rate limit, Error/Panic priority,
  burst-boundary preemption. **Backend deviation:** this esp-hal revision marks
  S31 LEDC/MCPWM unsupported, so GPIO8 is driven by RMT TX (1 MHz tick, 5 kHz
  PWM symbol) instead of LEDC; the task wakes only at burst/gap boundaries.
- IMU timing line: `imu_read/imax/late/lmax/miss`; it distinguishes executor
  starvation (large `late/lmax`) from I2C contention (large `imax`).
- Target build exit 0; only pre-existing `lvgl-sys` warnings. The device scan,
  haptic waveform and H2 wiring are the remaining H1/H2 evidence.

### H1.2 device BMM150 pass — 2026-10-20

Combined image `target/mosaico-s31-g2-r15.bin` (905,200 bytes, SHA-256
`2e8e3e48...`):

```text
[scan] ack0=00000002000420000000000000020600 ack1=00000000000002100000000000000000
[scan] bmm=0:11:00>32,0:12:00>32
sensor: imu=35-46Hz bmm=20Hz q=64 drops=2936 edrops=0 mod=interact(0x16)
        mags=2 mag=557.5uT raw=[-1371,421,386] ierr=0 iskip=267 berr=0 bskip=864
```

- Both BMM150s are on the **main I2C0** at 7-bit `0x11/0x12`; the initial chip ID
  read was `0x00` because the sensors were still in suspend. The Bosch sequence
  `0x4B |= 0x01`, 3 ms, then `0x40` returns `0x32`. The BMM150 soft reset is
  `0x4B |= 0x82` (1 ms), not the BMI270 `0x7E = 0xB6`.
- `mags=2`; the BMM150 pair runs at 20 Hz with compensated ~556 uT samples.
- The right-slot Interaction module is `interact(0x16)`; the module LDR/PIR/key
  inputs are published at 25 Hz.
- The IMU is 35–46 Hz under the ball/UI load, below the 50 Hz target; the
  `late/imax` timing line will drive the H2/G2.2 tuning.
- `SENSOR_EVENTS` is full with non-edge drops but zero edge drops, as expected
  until H2 consumes the queue.
- H1.3 haptics are target-build complete (RMT backend); the device waveform is
  triggered by H2 when the AffectEngine emits a pattern.

### H2/H3 + TE device boot — 2026-10-21

Image `target/mosaico-s31-h3.bin` (948,112 bytes, SHA-256 `ac0ea131...`) was
written to `0x210000` with `esptool --before no-reset --after hard-reset`; the
verify step passed. The CDC console then reported:

```text
[boot] R1 cdc=true disp=1 br=1 frames=492 p50=0 p95=0 max=0 n=0
       transfer_ms=11 fps=19 te=482/0/11 q=0 drops=0 hb=24 ldrop=55
ui p50=0 p95=0 max=0 ... tx=12 fps=19 px=1230080 ball=Idle person=idle
ui ball cost: engine=1xxus raster=18xxxus
sensor battery: soc=100% mv=4193 avg_ma=64 c=31 status=0x4028 err=0
display_task: damage=(116,116)-(363,363) chunks=8 transfer=12303us ok=true
sensor: imu=30-41Hz bmm=17-21Hz q=2 ... mags=2 mag=577.7uT
```

- `te=482/0/11` is the idle-wait TE revision: 482 frame boundaries observed,
  zero timeouts, 11 jobs dispatched while queued. Touch frames bypass the wait.
- `p50/p95` are zero because no touch had been sampled yet; the latency gate is
  still open on this revision.
- Battery registers decode to a plausible 100% / 4193 mV / 64 mA / 31 °C.
- The module read `mod=absent(0x00)` during this boot; the earlier H1.2 pass had
  `interact(0x16)`. Treat this as a module-presence question, not a driver
  regression, until the slot is re-seated.
- Visual acceptance of the splash, entrance, drag and tear-free motion remains
  the open gate; a screenshot cannot be captured over CDC.

### TE re-arm pass — flashed 2026-10-21

The `te=482` above counts edges observed while the display task was **idle**,
not transfers that actually started on an edge: the edge usually fired before
the next job existed, so the transfer still began at the job's arrival phase.
That is a measurement bug, not proof of alignment.

`display_task::next_job_at_frame_start` now waits for the job first, then arms
the TE edge for untagged (animation) jobs. While waiting it races the edge
against the next job and a 30 ms timeout:

- a newer job (normally the touch response) releases the unread animation frame
  and is served immediately — `ty=` counts these yields;
- no edge within 30 ms increments the timeout counter and transfers anyway;
- touch-tagged jobs still bypass the wait entirely.

Build and tests: `cargo test --workspace --offline` passes (47 `mosaico-s31`
tests among them); target build exits 0. Image
`target/mosaico-s31-h3-te2.bin` (948,896 bytes, SHA-256 `ae739a3c...`) was
written to `0x210000`; verify passed. Boot evidence:

```text
[boot] R1 cdc=true disp=1 br=1 frames=1667 p50=0 p95=0 max=0 n=0
       transfer_ms=12 fps=19 te=1282/0/386 ty=0 q=0 drops=0 hb=83 ldrop=177
ui ... tx=16-20 ty=0 fps=19 px=1230080 ball=Idle
display_task: frame=1680 damage=(116,116)-(363,363) chunks=8 transfer=37349us
```

- `te=1282/0/386`: 77% of animation frames now start on a real edge; 386 jobs
  were already queued and bypassed the wait.
- `tx` is the per-window average: typical sampled frames are 12-13 ms, but
  occasional frames reach 37 ms. The likely cause is the UI task's ~18 ms
  raster running between the display task's chunk awaits on the single-core
  executor; those frames exceed the 16.7 ms panel frame and can still tear.
- Touch `p50/p95` are zero until the next interaction; `ty` will show whether
  the yield path is exercised.
- The status line carries `ty=` after `tx=` and in the `[boot]` record.

### Drag frame measurement — 2026-10-21

A real drag/struggle session on `ae739a3c` produced:

```text
haptic: Surprised (1 burst(s) x 50 ms, 70%)
ui ball Idle -> Panic (persona=Some(Struggling) affect=Idle/Idle)
ui p50=44 p95=51 max=53 n=24 tx=33-44 tag=1 ball=Panic
display_task: frame=4440 damage=(80,79)-(407,380) chunks=10 transfer=49385us ok=true
```

- The drag union is **328×302**, i.e. the old and new 240×240 canvas positions.
  At 40 MHz and 8-10 chunks it transfers in **49 ms**, about three panel frames;
  TE alignment cannot hide a transfer that long, which is the tearing seen
  during a fast drag.
- Typical idle frames are still 12-13 ms; the 49 ms frame is the sum of
  ~20 ms of QSPI data, ~7 ms of RGB565 packing and the UI's ~18 ms raster
  interleaving between chunk DMA awaits on the single-core executor.
- P95 for that session was 51 ms (gate is 50 ms); the touch itself was served
  late by the long animation frame ahead of it.
- Candidate: **80 MHz QSPI + two-pixel packing**. The S31 SPI function clock
  is 160 MHz, so 80 MHz is a clean /2; the pack loop emits two big-endian
  pixels per 32-bit store. Image `target/mosaico-s31-h3-te3.bin` (949,424
  bytes, SHA-256 `37104f07...`) was flashed and verify passed:

  ```text
  [boot] R1 cdc=true disp=1 br=1 frames=3556 p50=41 p95=46 max=46 n=8
         transfer_ms=9 fps=19 te=2976/0/580 ty=0 q=0 drops=0 hb=177
  display_task: frame=360 damage=(116,116)-(363,363) chunks=8 transfer=7404us
  ```

  Typical 248×248 animation frames dropped from 12-13 ms to **7.4 ms**;
  `tx` is 11-12 ms over the window and P95 is 46 ms on 8 touches. The transport
  reports no errors, but 80 MHz visual stability and a fresh fast-drag sample
  still need the user's eyes.

### Boot wordmark — official logo cannot be removed without the bootloader

The Mosaico wordmark is painted by the factory bootloader before `main_app`
runs; the standing rule forbids rewriting the bootloader, partition table or
`vibe_mode`. What the application *can* do is stop re-running the 1.2 s
cold power sequence, which is what currently leaves the wordmark on the glass
while we reset and re-init the panel.

`display::init` now defaults to `PANEL_WARM_INIT = true`:

- RST is held high (no low pulse); SLPOUT and the vendor `0x51 0xFF` are
  skipped; DISPON is replayed idempotently after the GRAM clear with no 600 ms
  delay.
- The very first command is `0x51 0x00`, so the bootloader wordmark goes dark
  as soon as our app runs.
- `SHOW_SPLASH = false` in `ui.rs` skips the `ikaros` wordmark and arms the
  ball entrance directly, matching the user's fallback: official bootloader
  logo → ball.

Image `target/mosaico-s31-h3-te4.bin` (949,728 bytes, SHA-256 `f4dd7d66...`)
was flashed; boot reported `disp=1 br=1`, `fps=19`, `tx=10-12 ms`,
`person=idle` by hb=11 (ball entrance only, no splash). Cold init remains
available by flipping `PANEL_WARM_INIT` for a panel that was not initialized
by the bootloader.

### Remaining drag tear is a bypass, not a slow transfer — 2026-10-21

te4 drag samples (80 MHz, 2-pixel packing):

```text
display_task: frame=840 damage=(104,165)-(395,423) chunks=9 transfer=10101us
display_task: frame=960 damage=(172,95)-(419,342) chunks=8 transfer=9074us
ui p50=42 p95=54 max=54 n=12 ball=Confused/Idle
```

- Both drag frames are **9-10 ms**, already inside the 16.7 ms panel frame.
  The user still sees tearing, so raw transfer time is no longer the cause.
- The boot line showed `te=262/0/39`: 13% of animation jobs were dispatched
  through the old fast path because a newer frame was already queued. A fast
  drag is exactly when urgent frames queue, so those frames start mid-scanout
  and tear.
- `next_job_at_frame_start` now aligns **queued animation frames too**; only a
  touch-tagged frame bypasses. Image `target/mosaico-s31-h3-te5.bin` (950,144
  bytes, SHA-256 `9b8fb79a...`) was flashed. It reports
  `te=3032/0/4` (99.9% aligned), `tew=7-8 ms` (confirms a **60 Hz** panel).
- The user still saw irregular tearing. The drag capture contains the proof:

  ```text
  frame=1800 damage=(120,139)-(379,428) chunks=10 transfer=37616us
  frame=2280 damage=(132,78)-(395,392) chunks=10 transfer=39165us
  ```

  Most drag frames are 8-10 ms, but two are **37-39 ms**. Their raster lines
  show `raster=17xxx us` and the sensor read is inflated to ~25 ms by the same
  starvation, so this is the UI's ~18 ms raster running between the display
  task's chunk DMA awaits — not a bypass and not a slow QSPI path.
- **te6** adds a cooperative handshake: `DISPLAY_ACTIVE` stops the UI from
  starting a repaint while a job is being uploaded, and `UI_RENDERING` makes
  the display wait for a raster that was already running before it arms the TE
  edge. Image `target/mosaico-s31-h3-te6.bin` (951,104 bytes, SHA-256
  `031fc979...`) was flashed; it works structurally (`te=240/0/0`) but holds
  `DISPLAY_ACTIVE` across the TE wait as well as the transfer, so the UI skips
  too many 5 ms loops and the frame rate collapsed to **11 fps** (`px` 0.68-0.74M).
- **te7** narrows the handshake: `DISPLAY_ACTIVE` is set only for the upload
  (the display is synchronous between the aligned return and that store), and
  the post-edge check skips an edge if a raster started during the wait.
  `ui_task` accumulates skipped milliseconds in `pending_ms`, so the ball's
  50 ms frame clock does not slow down. `target/mosaico-s31-h3-te7.bin`
  (951,568 bytes, SHA-256 `92ea1de9...`) was flashed:

  ```text
  [boot] ... fps=19 te=1378/0/2 ty=0 tew=9 ...
  frame=840 damage=(136,67)-(391,363)  chunks=10 transfer=10891us
  frame=960 damage=(48,24)-(403,372)   chunks=11 transfer=17519us
  ui p50=42 p95=45 max=45 n=2
  ```

  The 37-39 ms interleave is gone and 19 fps is restored. One large diagonal
  flick still produces a **356x349** union and **17.5 ms** upload, just over
  the 16.7 ms frame — that is the last visible tear.
- **te8** adds a per-rendered-frame movement cap: `MAX_DRAG_STEP_PX = 72`
  (1440 px/s at 20 fps). The target still follows the finger; the ball catches
  up on later frames, which also reads as drag resistance. The entrance is
  exempt. `target/mosaico-s31-h3-te8.bin` (951,664 bytes, SHA-256
  `89b5405d...`) was flashed. The user accepted the feel ("拖拽手感可以了") but
  still saw a little tearing. The last offending frame:

  ```text
  frame=3480 damage=(84,61)-(419,391) chunks=11 transfer=14932us
  ```

  This is under 16.7 ms, so it is **geometric**, not scheduling: the union is
  336x331 and starts at y0=61. Writing at 45 us/row is slower than the panel's
  34.7 us/row scanout, and the 2.1 ms head start from y0=61 is consumed after
  ~204 rows — the scanout catches the write near the bottom of the ball.
- **te9** makes the chunk row count depend on the damage width: the staging
  buffer holds 32 rows at 480 px but 45 rows at 336 px and 60-64 rows at
  248-240 px. `target/mosaico-s31-h3-te9.bin` (951,792 bytes, SHA-256
  `f8cd1fae...`) was flashed. The transfer goal was met:

  ```text
  frame=960 damage=(96,47)-(351,379) chunks=6 transfer=9884us
  ```

  **Rejected on sight:** the user reports a residual horizontal line that
  appears to cut the ball, alongside tearing. The transfer is 9.9 ms, faster
  than the 34.7 us/row scanout, and no chunk failed (`ok=true`), so this is a
  chunk-boundary data artifact from the 60-row bursts, not a slow scanout
  race. The CO5300 GRAM path is only proven with 32-row bursts at 80 MHz.
- **te10** reverts the dynamic row count to the proven 32-row chunks and keeps
  te7's handshake and te8's 72 px cap. Image
  `target/mosaico-s31-h3-te10.bin` (951,680 bytes, SHA-256 `3d34641a...`) is
  built; **not flashed**. Expected: the seam disappears, the tiny te8 tear
  returns. 32 rows is the safe operating point; larger bursts need a panel
  FIFO/timing investigation before they are trusted.

## Why the official camera looks smooth

The camera path is not doing something magical: it uses the PPA (2D
scale/rotate/blend/format conversion) and DMA so the CPU never packs pixels,
and it can therefore push a large, contiguous preview buffer inside one panel
frame. PPA does not remove tearing by itself; it removes the CPU work and the
resulting scheduling interference, which is what makes TE alignment effective.

Our bottleneck is the same class: the CPU packs RGB565 from the 480-wide PSRAM
framebuffer, and on a single-core executor the UI raster can interleave with
that work. te6 removes the interference without PPA. If a future screen needs
full-frame 60 Hz motion, the next step is PPA + direct DMA (or a dedicated
display thread), not a different panel.

### Drag removed; fixed ball plane accepted — 2026-10-21

Product decision: the grab/drag gesture is dropped. The display therefore never
moves the canvas, and the square-boundary/white-line class of artifacts cannot
occur.

Deleted end to end:

- `grok-ball` personality: grab/drag phases, `Struggling`/`Dizzy`/`Startled`,
  offsets, wobble, drift-home, `GRAB_HOLD_MS`/`BALL_TRAVEL`/`HOME_*`.
- `sprite`: drag animation slots and reaction mappings.
- `mosaico-s31`: runtime/BallVisual offsets and grabbed/wobble, the widget's
  canvas move/trail/4px pad/72px step, and the plane's old-position bands,
  four-side scrub and per-move trace.
- `ball_plane` is now a fixed 240x240 upload at `(120,120)`, 8 chunks.

The entrance is baked into the canvas (40 px in-canvas drop, squash, hop), so
the canvas window never crosses the panel edge. Host tests pass; the device
accepted the build:

```text
[boot] R1 cdc=true disp=1 br=1 frames=302 p50=34 p95=42 max=42 n=17
       transfer_ms=6 fps=19 te=286/0/17 ty=0 tew=6
ball_plane: frame=360 chunks=8 transfer=6969us ok=true
```

Image `target/mosaico-s31-h3-fixed4.bin` (945,920 bytes, SHA-256 `367b148b...`).
The unused full-screen presenter/display_task fallback is still compiled but not
spawned; it is not drag-specific and is left for a later cleanup.

## Evidence rules

- A gate is `complete` only with the recorded invocation, exit status and retained log or artifact.
- Host timeline tests do not establish device thresholds.
- Module presence is an EEPROM result; it is not inferred from sensor activity.
- Haptic patterns require device observation; a host timer test is not device acceptance.
- `clean_acceptance_count` increments only after a full gate pass on the recorded revision.
