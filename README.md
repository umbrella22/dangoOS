# ESP-Mosaico S31 — grok-ball first screen

Rust firmware and a host-testable core for the ESP-Mosaico v1.2 display terminal
(ESP32-S31, CO5300 480×480 QSPI AMOLED, CST9220 touch). The UI layer is
[`lv_binding_rust/lvgl-ui`](../lv_binding_rust/lvgl-ui/README.md); the first
screen is a Rust port of the MIT-licensed grok-ball engine.

## Workspace

| Path | Responsibility |
| --- | --- |
| `crates/grok-ball` | `no_std` data, ball engine, RGB565 rasterizer, sensor affect engine, touch personality, character skins and sprite playback; 110 host tests |
| `crates/mosaico-s31` | S31 firmware: `esp-rtos`/Embassy tasks, CO5300/CST9220/sensor drivers, runtime arbiter (affect + personality), `BallWidget` with a two-slot canvas pool, and the host previews |
| `../lv_binding_rust/lvgl-ui` | Declarative `App`/`Node`/`Ui` layer plus `CustomWidget` and `Ui::tick` |
| `archive/crates-pre-grok-ball-20261020` | Archived single-threaded S31 prototype; not a workspace member |
| `archive/crates-pre-s31-20261019` | Archived Slint/fan crates; not a workspace member |

## Commands

Run from the repository root:

```bash
# Host core: data parity, engine, raster, affect timelines
cargo test -p grok-ball --offline

# Six 320x320 PPM frames from the grok-ball core (Idle/Listening/Thinking/Speaking/Happy/Error)
cargo run -p grok-ball --example states --offline

# 16-expression BallWidget preview on a 480x480 host display
cargo run -p mosaico-s31 --example preview --offline

# Boot wordmark, entrance drop and cat/fox skins as PPM frames
cargo run -p mosaico-s31 --example splash_preview --offline

# Firmware target build
cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf

# UI layer tests in the sibling binding repository
cargo test -p lvgl-ui --manifest-path ../lv_binding_rust/Cargo.toml -- --test-threads=1
```

If the crates.io index is slow, prefix the commands with
`CARGO_NET_OFFLINE=true`; the dependency cache and `Cargo.lock` are complete.

## Current status

- **G0 complete:** 25 eye-ring sets, 96-point body, 32 seeds, 8 data-parity
  tests, 10 engine tests, 7 raster tests. The six-state frames are written under
  `target/grok-ball-states/`.
- **H0 complete:** 56 affect timeline tests for shake, touch, light, PIR, keys,
  BMM150, module hot-plug, haptics, and the `Activity/Error` input.
- **G1 complete:** `CustomWidget`/`Ui::tick` in `lvgl-ui` (51 tests) and the
  `BallWidget` host preview with 14 expressions and a single create/delete
  lifecycle. Frames are written under `target/ball-widget-preview/`.
- **R0 complete on hardware:** USB-OTG HS CDC console, continuous `ui_task`
  heartbeats, full-frame 480×480 transfer, black field plus the color bar, and
  touch down/release logs. Verified image `target/mosaico-s31-r0.bin`,
  167,856 bytes, SHA-256 `6b0ca615...`.
- **R1 complete on hardware:** async QSPI + two-frame presenter, GPIO6/async I2C
  input, LVGL `ui_task`. 100-touch firmware metric: P50 29–33 ms, P95 36–41 ms,
  max 101 ms, zero queue drops, 76-second run without reset. Verified image
  `target/mosaico-s31-r1.bin`, 764,448 bytes, SHA-256 `e362f5a9...`.
- **G2 complete on hardware:** the grok-ball first screen on a fixed 240×240
  ball plane at the panel centre, firmware P95 30 ms, 20 fps. (At G2 a tap
  cycled the six AI states; that demo binding has since been replaced, see
  below.)
- **H1.2 sensor hardware online:** both BMM150s on main I2C0 `0x11/0x12`
  (chip ID `0x32`, 20 Hz, ~556 µT), right-slot module `interact(0x16)`, IMU
  35–46 Hz, zero edge drops.
- **Pending:** H2 AffectEngine + RMT haptics, IMU 50 Hz tuning, G3 optional full
  catalog/effects, and the later Hub/C3/voice/Lua work.

## Current firmware behavior

- Touch goes to the grok-ball personality engine (tap ladder, grab, drag), and
  sensor events drive the affect engine. `runtime::merge_expression` decides
  the shown expression: urgent affect (Error, Panic) > personality reaction >
  affect expression > resting expression. Haptics follow the same order.
- Display: LVGL only hosts the widget lifecycle and never renders the screen.
  `BallWidget` rasterizes into a two-slot canvas pool on the UI task;
  `ball_plane_task` uploads the newest slot over async QSPI from a priority-2
  interrupt executor. `ui::SHOW_SPLASH` must stay `false` on this path
  (compile-time check).
- The latest device image is `target/mosaico-s31-h3-fixed4.bin` (fixed ball
  plane, see [QUICKSTART](QUICKSTART.md)). Later commits (expression
  priority, canvas pool, interrupt executor) build and pass host tests but
  still need a hardware run.

## Build environment

- `../lv_binding_rust` is a path dependency. The tested revision is recorded
  in `lvgl.rev`; `scripts/check-lvgl-pin.sh` warns if the sibling checkout
  differs or is dirty.
- `.cargo/config.toml` sets the Espressif C toolchain (`CC_…`, `AR_…`),
  `LIBCLANG_PATH` and `LVGL_SYSROOT` for the author's machine. Cargo's `[env]`
  does not override variables that are already set, so on another machine
  export your own values (for example in your shell profile) instead of
  editing the file:

  ```bash
  export ESP_TOOLS=$HOME/esp-tools/riscv32-esp-elf
  export CC_riscv32imafc_unknown_none_elf=$ESP_TOOLS/bin/riscv32-esp-elf-gcc
  export AR_riscv32imafc_unknown_none_elf=$ESP_TOOLS/bin/riscv32-esp-elf-ar
  export LVGL_SYSROOT=$ESP_TOOLS/riscv32-esp-elf
  ```

## Firmware flash path

Build the image, then write only the `main_app` region:

```bash
cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
espflash save-image --chip esp32s31 \
  target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw \
  target/mosaico-s31-fw.bin

esptool --chip esp32s31 --port <PORT> --baud 115200 \
  --before no-reset --after hard-reset \
  write_flash 0x210000 target/mosaico-s31-fw.bin
```

Verified images:

- R0: 167,856 bytes, SHA-256
  `6b0ca61529d2cb3f5bb130d6fd6f0155404e80f2a18e5518c9af7f1e8ab04f13`.
- R1: 764,448 bytes, SHA-256
  `e362f5a9bd40327f0bef9eb8748c79b7df340c2ef06ae5bbf9940fab92cb4785`.
- G2.1/G2.2 (ball + sensors): 905,200 bytes, SHA-256
  `2e8e3e48...`; it also carries the BMM150 power-up fix and the persistent
  I2C scan.

Do **not** rewrite the bootloader, the partition table or `vibe_mode` at
`0x20000`. The official v0.5.0 layout keeps `main_app` at `0x210000`.

Console:

```bash
espflash monitor --chip esp32s31
# or: espflash monitor --chip esp32s31 --port /dev/cu.usbmodem1101
```

The console is a USB-OTG HS CDC-ACM device (`Usb::new_hs` + `embassy-usb`), not
USB-Serial-JTAG. The verified R0 image enumerates as
`/dev/cu.usbmodemmosaico_s31_r01` and replays a boot summary every 5 seconds:

```text
[boot] Mosaico S31 R0 cdc=true drops=0 q=0/32 disp=true chunks=30 ms=38 hb=10
```

`disp=true` means the full frame was transferred; `hb` is the current `ui_task`
heartbeat. `input_task` logs `touch down at (x, y)` and `touch release`. Use
`espflash monitor --chip esp32s31 --port /dev/cu.usbmodemmosaico_s31_r01`.

## Documents

- [Execution order](docs/plans/ROADMAP.md) — what runs first and what is blocked
- [RFC 0003 grok-ball first screen](docs/plans/grok-ball-first-screen-plan.md)
- [RFC 0004 sensor reactions](docs/plans/grok-ball-sensor-reactions-plan.md)
- [RFC 0005 platform runtime](docs/plans/platform-runtime-plan.md)
- [Mosaico v1.2 hardware notes](docs/MOSAICO_V12_HARDWARE.md)
- [Legacy fan/Slint material](docs/legacy/README.md)
