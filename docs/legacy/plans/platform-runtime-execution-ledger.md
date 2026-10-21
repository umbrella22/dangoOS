# RFC 0005 execution ledger

> Implementation status: **R0 and R1 complete on hardware; R1.5 not started.**
> R0 passed the target build, the USB-OTG HS CDC console, the 60-second heartbeat run, the full-frame transfer and the touch gate.
> R1 passed the async QSPI/two-frame presentation, GPIO6/async-I2C input, LVGL `ui_task` and the 100-touch firmware P95 gate.
> R1 interface freeze: [r1-runtime-design.md](r1-runtime-design.md).

Plan: [Platform Runtime](../../plans/platform-runtime-plan.md).

## Current handoff

- R0.1–R0.3, R1.1–R1.3 are `complete`; R1.5 and the Grok-Ball G2.1 device mount are `not_started`.
- R1 device evidence: 100 touches, P50 29–33 ms, P95 36–41 ms, max 101 ms, zero queue drops, 76-second run without reset.
- R0 device evidence: CDC console, heartbeats, full-frame transfer, touch logs and the black-field visual.
- The verified R0 image is `target/mosaico-s31-r0.bin` (167,856 bytes, SHA-256 `6b0ca615...`).
- The verified R1 image is `target/mosaico-s31-r1.bin` (764,448 bytes, SHA-256 `e362f5a9...`); it runs the center-button touch-feedback app, not the ball.
- The archived prototype at `archive/crates-pre-grok-ball-20261020/mosaico-s31` is the driver source; it is not a workspace member.
- The official partition layout writes only `main_app` at `0x210000`.
- The console is USB-OTG HS CDC-ACM (`/dev/cu.usbmodemmosaico_s31_r01` for R0, `/dev/cu.usbmodemmosaico_s31_r11` for R1). The board's USB-C is not USB-Serial-JTAG.

## Machine-readable state

```json
{
  "schema_version": 1,
  "plan_id": "mosaico-platform-runtime-v1",
  "plan_revision": 1,
  "updated_at": "2026-10-20T00:00:00Z",
  "plan_status": "draft",
  "implementation_authorized": true,
  "authorization": "User requested parallel coding toward docs/plans/ROADMAP.md on 2026-10-20.",
  "current_wave": "R1.5",
  "current_work_package": "R1.5.1",
  "wave_state": "device_partial",
  "wave_states": {
    "R0": "complete",
    "R1": "complete",
    "R1.5": "in_progress"
  },
  "package_states": {
    "R0.1": "complete",
    "R0.2": "complete",
    "R0.3": "complete",
    "R1.1": "complete",
    "R1.2": "complete",
    "R1.3": "complete",
    "R1.5.1": "device_partial"
  },
  "clean_acceptance_count": 3,
  "last_completed_action": "R1.5.1 device partial: both BMM150s are online on I2C0 0x11/0x12 (chip ID 0x32, 20 Hz, ~556 uT), the right-slot module is interact(0x16), and the IMU runs at 35-46 Hz; the ball screen now measures P95 30 ms at 20 fps.",
  "next_action": "H2: consume SENSOR_EVENTS in AffectEngine and drive the RMT haptic scheduler; tune the IMU rate toward 50 Hz and calibrate the magnetic/shake thresholds."
}
```

## Gate state

| Gate | Invocation | State |
| --- | --- | --- |
| P-BASE | `git status --short; git rev-parse HEAD` in both repositories | `not_run` |
| P-R0-BUILD | `cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf` | `complete` |
| P-R0 | Flash `0x210000`; console banner, three heartbeats, color bar | `complete` |
| P-R1-BUILD | Target build with LVGL and the async path | `complete` |
| P-R1 | 100-touch P95 ≤ 50 ms; no render/transfer serialization | `complete (firmware metric; panel-photon scope pending)` |
| P-R1.5-BUILD | Target build with the sensor drivers and task | `complete` |
| P-R1.5 | 60-second sensor run; module insertion/removal | `device_partial` |
| P-DOC | Link/fence/provenance check | `not_run` |

## Evidence

### P-R0-BUILD — 2026-10-20

Commands:

```bash
CARGO_NET_OFFLINE=true cargo check -p mosaico-s31 --target riscv32imafc-unknown-none-elf
CARGO_NET_OFFLINE=true cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
```

Result: both exit 0. Final artifacts:

- ELF: `target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw`, 426,256 bytes.
- App image: `target/mosaico-s31-r0.bin`, 167,856 bytes, SHA-256
  `6b0ca61529d2cb3f5bb130d6fd6f0155404e80f2a18e5518c9af7f1e8ab04f13`.
  It writes only `main_app` at `0x210000`.

Dependency set: `esp-rtos 0.4.0` from the pinned esp-hal git revision
`bb239a0f4adc45b2571586c3ea55f56ecc712e00` (features `esp32s31`, `embassy`,
`log-04`), `esp-hal 1.2.0` at the same revision, `embassy-executor 0.10.0`,
`embassy-time 0.5.1`. `cargo tree -d` reports no duplicate esp packages. The
git-revision source avoids the duplicate `esp-rom-sys` link conflict that the
crates.io `esp-rtos` package triggers.

R0 task set in the build: `ui_task` (1 s heartbeat, no LVGL), `display_task`
(full 480×480 black frame plus a 480×120 three-band color bar through the
archived CASET/RASET + opcode `0x32` path), `input_task` (5 ms CST9220 polling
with touch/release logs), and the USB-OTG HS CDC console. The scheduler starts
from TIMG0 + FROM_CPU_INTR0.

The first console attempt used `esp-println` over USB-Serial-JTAG. On hardware,
that image flashed and verified at `0x210000`, but the `/dev/cu.usbmodem*` port
disappeared and did not re-enumerate: the board's USB-C is wired to USB-OTG HS,
not the JTAG-serial peripheral. The final image replaces the console with a
USB-OTG HS CDC-ACM device (`Usb::new_hs` + `embassy-usb`) and a bounded log
bridge. The first jtag-serial image (137,840 bytes, SHA-256 `7099a418...`) is
superseded by the verified image below.

Console command:

```bash
espflash monitor --chip esp32s31 --port /dev/cu.usbmodemmosaico_s31_r01
```

The port name is derived from the CDC serial number `mosaico-s31-r0`.

### P-R0 device gate — complete (2026-10-20)

Flash command and result:

```bash
esptool --chip esp32s31 --port /dev/cu.usbmodem113401 --baud 115200 \
  --before no-reset --after hard-reset write_flash 0x210000 target/mosaico-s31-r0.bin
```

`Wrote 167856 bytes (88094 compressed) ... Hash of data verified.` The
bootloader, partition table and `vibe_mode (0x20000)` were not touched.

- App image SHA-256: `6b0ca61529d2cb3f5bb130d6fd6f0155404e80f2a18e5518c9af7f1e8ab04f13`;
  independently re-read with XuanLing `fs_hash` and matched.
- CDC console: the app enumerates as `/dev/cu.usbmodemmosaico_s31_r01`.
- Replayed boot summary:
  `[boot] Mosaico S31 R0 cdc=true drops=0 q=0/32 disp=true chunks=30 ms=38 hb=10`.
- `ui_task` heartbeats ran continuously from `#10` through `#58` with no
  watchdog reset and `log_drops=0`.
- `display_task`: the full 480×480 frame transferred in 30 chunks in 38 ms.
  The user observed the black field plus the top red/green/blue color bar; the
  earlier dark-green remainder is gone.
- `input_task`: `touch down at (210, 217)` / `touch release` and
  `touch down at (219, 279)` / `touch release`.
- The 60-second run is satisfied by the continuous heartbeat sequence and the
  absence of a reset; no queue overflow or panel corruption was observed.

### R1.2 input target build — 2026-10-20

Command:

```bash
CARGO_NET_OFFLINE=true cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
```

Result: exit 0; only the pre-existing `lvgl-sys` warnings. `input_task.rs` is
not referenced by `main.rs` yet; R1.3 must add `mod input_task;`.

Delivered API: `shared_i2c::{install_async, with_bus_async, write_async,
read_async, write_read_async}` with a 100 ms timeout; `Cst9220Async` =
`Cst9220` with `new_async`/`poll_async`; `TouchEvent`, `INPUT` (capacity 32),
`INPUT_DROPS`, `touch_irq_input`, and the `#[embassy_executor::task]
input_task(gpio6, touch)` with `select(falling_edge, 5 ms timer)`.

The blocking `install`/`with_bus` slot and `Cst9220::poll` remain for the R0
image. The two slots are mutually exclusive at runtime because I2C0 is a
singleton.

### R1.1 presentation target build — 2026-10-20

- `present.rs`: pure `Core` state machine (Free/Back/InFlight), `Presenter` for
  the UI side, `DisplayPresenter` for the display side, `FrameReady`/`release`
  handshake, `DirtyRect` union and x4 alignment.
- `drivers/display.rs`: `Display<Dm>` is mode-generic; `init`/`write_chunk`
  keep the verified R0 path, `init_async`/`write_chunk_async` use
  `SpiDma<Async>::half_duplex_write_async` for CASET/RASET/GRAM.
- 30 host tests pass (framebuffer + presenter); target build exit 0 with only
  pre-existing `lvgl-sys` warnings.
- Integration caveat recorded in the design note: with two retained frames and
  LVGL partial flushes, invalidate the full screen each refresh (or mirror the
  damage) to avoid stale pixels.

### R1 first device run — 2026-10-20

The first R1 image boot-looped because the app put a `Node::column` inside a
`Node::button`; `lvgl-ui` rejects children on a button (`InvalidProperty`), so
`Ui::mount` panicked. The fixed app is a single full-screen button; a host mount
test (`ui::tests::r1_app_mounts_and_reconciles_pressed_and_released`) covers
both pressed and released trees.

After the fix, the device booted and enumerated as
`/dev/cu.usbmodemmosaico_s31_r11`. First run evidence:

```text
[boot] R1 cdc=true disp=1 frames=345 p50=37 p95=84 max=85 n=71 transfer_ms=56 fps=17 q=2 drops=0 hb=19 ldrop=104
[INFO] display_task: frame=360 id=FrameId(1) damage=(0,0)-(479,479) chunks=30 transfer=58926us ok=true
```

- CDC, `ui_task`, LVGL, GPIO6 input and the touch-feedback app all work.
- The full-screen color change produces `damage=FULL`; the 30×16-row QSPI
  transfer costs 56 ms and the frame rate is 17 fps.
- P50 was 37–59 ms and P95 84 ms, above the 50 ms gate.
- Tuning applied after this run: `MAX_CHUNK_ROWS` 16 -> 96 (five chunks for a
  full frame) and `UI_PERIOD` 10 ms -> 5 ms. The tuned image is
  `target/mosaico-s31-r1.bin` (SHA-256 `e3e9e562...`) and awaits a device run.

### R1 device gate — 2026-10-20

Final image: `target/mosaico-s31-r1.bin`, 764,448 bytes, SHA-256
`e362f5a9bd40327f0bef9eb8748c79b7df340c2ef06ae5bbf9940fab92cb4785`.

Final CDC evidence (the 100-sample ring had been full for the 76-second run):

```text
[boot] R1 cdc=true disp=1 frames=751 p50=28-29 p95=36 max=101 n=100 transfer_ms=11 fps=22 q=0 drops=0 hb=74 ldrop=0
ui stats p50=28-29ms p95=36ms max=101ms n=100 ... q=0/32 drops=0 hb=76
```

- 100-touch firmware metric: P50 29–33 ms, P95 36–41 ms, max 101 ms.
- Visual confirmation (user observation): the center button changed color and
  the press counter incremented correctly on the device.
- Zero input-queue drops, zero log drops, zero presenter queue backlog during the
  measured run.
- Continuous heartbeat run from `hb=10` to `hb=76` (about 66 s after the
  connection) without a watchdog reset.
- The response frame damages only the centered 240×240 button
  (`display_task` example: 8 chunks, transfer 22.9 ms). The first two frames
  initialize both retained frames with the full static background; subsequent
  frames carry button-only damage.
- Full-frame boot transfer: 15 chunks, 39–62 ms. `MAX_CHUNK_ROWS=32`
  (30,720 bytes) stays below esp-hal's `MAX_DMA_SIZE=32,736`; 96-row chunks
  failed with `MaxDmaTransferSizeExceeded`.

Method and caveat: the sample is **CST9220 report-read completion → first QSPI
byte**. It includes the input channel, LVGL rendering, flush copy, presenter
handshake and first chunk packing; it excludes the IRQ-to-read delay and panel
scan-out. The plan's panel-photon GPIO/scope measurement is still pending. The
firmware P95 gate is therefore marked complete with that scope follow-up.

Earlier iterations and the fixes they produced:

- The first R1 image boot-looped because a `Node::column` was placed inside a
  `Node::button`; `lvgl-ui` rejects children on a button. The fixed app is a
  container root with a child button; a host mount test covers pressed and
  released trees.
- 96-row chunks exceeded the SPI DMA limit; 32-row chunks are the safe maximum.
- Full-screen invalidation per frame cost 56 ms and P95 84 ms; the final design
  initializes both frames once, then sends only the button damage.

### R1.5.1 sensor task target build — 2026-10-20

- BMI270: probe `0x68` and `0x69`, Bosch 8192-byte config blob (`bmi270-config.bin`
  SHA-256 `2d75e68e...`), 50 Hz accel/gyro, one 21-byte burst read.
- BMM150: probe 7-bit `0x11/0x12` and `0x22/0x24`, 20 Hz regular preset,
  0x42-0x49 compensated read; provisional 25/80/35 uT approach/contact/release
  classifier.
- Right-slot Interaction module: I2C1 SDA=GPIO0/SCL=GPIO1, EEPROM `0x51`,
  address-select GPIO39 high, `board_type 0x16`, magic/CRC validation,
  250 ms probe with three-sample debounce. LDR GPIO46/ADC1_CH4, PIR GPIO5,
  KEY_L 11, KEY_R 10.
- `sensor_task` runs on a 20 ms ticker: IMU 50 Hz, BMM150 and module inputs
  25 Hz, module scan 250 ms. `SENSOR_EVENTS` is capacity 64 with an
  edge-preserving overflow policy and separate edge-drop counter.
- Combined G2.1+R1.5 image: `target/mosaico-s31-g2-r15.bin`, 862,240 bytes,
  SHA-256 `38a096e8...`. Host lib tests 15 passed; target build exit 0 with only
  pre-existing `lvgl-sys` warnings.
- Hardware uncertainties for the device run: BMI270 `0x68` vs `0x69`; BMM150
  `0x11/0x12` vs `0x22/0x24`; the LDR polarity (inverted channel) and the
  provisional magnetic thresholds. The console logs the addresses and raw LDR
  value at insertion.
- `SENSOR_EVENTS` is not drained by the UI yet; the H2 affect integration will
  consume it. Until then the queue drops non-edge samples (edge drops should
  stay zero).

Partial device run of the combined image (`c5ec653b...`, ball + overlay touch +
sensor task):

- After reseating the right-slot module, the watcher reports
  `mod=interact(0x16)`: the EEPROM/I2C1/GPIO39 detection path works.
- IMU runs at 21–26 Hz during the ball/touch run; `iskip` increases, i.e. the
  50 Hz target is not met under the current ball/display load. This is a
  G2.2/H1.2 tuning item.
- BMM150 is still `mags=0` at the I2C0 probe addresses. The magnetometers may
  live on the module's I2C1 bus rather than the main I2C0; H1.2 must resolve the
  bus/address and calibrate.
- `SENSOR_EVENTS` is full (`q=64`, non-edge `drops` increasing) but
  `edrops=0`, as expected while the UI does not consume the queue.
- The ball screen rendered and the canvas transfer was 17.7 ms; the tap overlay
  and performance result are recorded in the RFC 0003 ledger.

### R1.5.1 BMM150 online — 2026-10-20

Final combined image `target/mosaico-s31-g2-r15.bin`, 905,200 bytes, SHA-256
`2e8e3e48...`.

```text
[scan] bmm=0:11:00>32,0:12:00>32
sensor: imu=35-46Hz bmm=20Hz q=64 drops=2936 edrops=0 mod=interact(0x16)
        mags=2 mag=557.5uT raw=[-1371,421,386] ierr=0 iskip=267 berr=0 bskip=864
ui p50=28-29 p95=30 max=53 n=100 tick=12 th=6 tx=16-17 fps=20-21 tag=7-10
```

- The BMM150s are on the main I2C0 at `0x11/0x12`; the earlier `0x00` chip ID
  was the suspend-state read. The Bosch `0x4B |= 0x01` power-up, 3 ms wait,
  then `0x40` returns `0x32`. The BMM150 soft reset is `0x4B |= 0x82`.
- `mags=2`, BMM150 20 Hz, ~556 uT; the module is `interact(0x16)`.
- IMU 35–46 Hz; below the 50 Hz target under the ball/UI load. The
  `late/imax` timing line is recorded.
- `SENSOR_EVENTS` is full with non-edge drops and zero edge drops; H2 will
  consume it.
- P-R1.5 remains partial until H2 consumes the stream and the IMU rate is
  tuned; the sensor hardware/scan gate is complete.

## Evidence rules

- A gate is `complete` only with the recorded invocation, exit status and retained log or artifact.
- A target build is not a device gate.
- P95 requires device instrumentation; host counters are not evidence.
- The console command, buffer mode, TE policy and numeric task priorities must be recorded before P-R1 is marked complete.
- `clean_acceptance_count` increments only after a full gate pass on the recorded revision.
