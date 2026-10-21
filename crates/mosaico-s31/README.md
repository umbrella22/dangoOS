# mosaico-s31 — ESP32-S31 firmware

ESP-Mosaico v1.2 (ESP32-S31) firmware crate for the RFC 0005 platform
runtime, plus host previews of the ball widget. The library half
(`src/lib.rs`) is host-testable; the firmware binary is `mosaico-s31-fw`.

## Tasks

`esp-rtos` + Embassy with two executors on core 0:

| Task | Executor | Behavior |
| --- | --- | --- |
| `ball_plane_task` | interrupt (FROM_CPU_INTR1, priority 2) | Sole owner of the CO5300. Takes the newest canvas-pool slot and uploads it at the panel centre in 32-row async QSPI DMA chunks; animation frames wait for TE, touch-tagged frames skip it. |
| `ui_task` | thread | Sole owner of LVGL. Every 5 ms: drain sensors and touch into the runtime arbiter, advance `BallWidget` (raster into a free pool slot), hand the frame to the plane, log stats once per second. LVGL never renders the screen. |
| `input_task` | thread | Sole owner of the CST9220. GPIO6 edge + 5 ms fallback, async I2C reads into a bounded queue. |
| `sensor_task` | thread | BMI270, two BMM150, right-slot Interaction module; bounded `SensorEvent` channel. |
| `haptic_task` | thread | Motor patterns on GPIO8 via RMT-generated 5 kHz PWM. |

Key modules: `runtime` (affect + personality merge, `merge_expression`),
`ball_widget` (LVGL custom widget), `canvas_pool` (two-slot lease pool shared
by the widget and the plane), `ball_plane`, `display_stats` (telemetry
atomics), `lvgl_port` (headless LVGL display registration).

The console is a USB-OTG HS CDC-ACM device. `log` records are formatted into
fixed 160-byte records and queued through a bounded
`embassy_sync::channel::Channel`; a pump writes them to the CDC port once the
host opens it. `esp-println` is not used for application logs: its
`jtag-serial` path is not wired to the USB-C connector on this board.

## Hardware truth (unchanged from the archived prototype)

- CO5300 QSPI AMOLED, 480x480, RGB565, 40 MHz, SPI2 + AXI DMA.
- QSPI register opcode `0x02`, GRAM write opcode `0x32`; `CASET`/`RASET`
  windows are inclusive and x must stay 4-pixel aligned.
- CST9220 capacitive touch at 7-bit I2C address `0x5A` on I2C0
  (SDA=56, SCL=3, 400 kHz). INT is GPIO6 (R1).
- VCC_3V3 (GPIO60) is active-low and must be on before panel/touch init.
- Official partition: write only `main_app` at `0x210000`. Do not rewrite the
  bootloader, the partition table, or `vibe_mode` at `0x20000`.

## Dependencies

- `esp-hal`, `esp-alloc`, `esp-bootloader-esp-idf`: pinned git revision
  `bb239a0f4adc45b2571586c3ea55f56ecc712e00`.
- `esp-rtos = 0.4.0`, features `esp32s31`, `embassy`, `log-04`, also taken from
  the same git revision. The `esp32s31` feature exists in the crate and forwards
  to `esp-hal/esp32s31`, `esp-alloc?/esp32s31` and
  `esp-metadata-generated/esp32s31`.
  The registry copy of esp-rtos was tried first and rejected by Cargo: it links
  a second `esp-rom-sys` (`links = "esp_rom_sys"`) alongside the pinned git
  copy. The git copy path-resolves every esp-* support crate at the same
  revision, so there is exactly one esp-hal/esp-rom-sys.
- `embassy-executor = 0.10` (resolved 0.10.0) and `embassy-time = 0.5`
  (resolved 0.5.1), the versions used by the pinned esp-hal examples and by
  esp-rtos 0.4.0.
- `embassy-usb = 0.6` (resolved 0.6.0, `default-features = false`),
  `embassy-futures = 0.1` (resolved 0.1.2) and `embassy-sync = 0.8`
  (resolved 0.8.0) for the USB-OTG HS CDC-ACM console. `embassy-usb 0.6.0`
  requires `embassy-usb-driver 0.2`, `embassy-sync 0.8`, `embassy-time 0.5`
  and `embassy-net-driver-channel 0.4`, all compatible with the pinned HAL.

## Build commands

```bash
cargo check -p mosaico-s31 --target riscv32imafc-unknown-none-elf
cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
```

If the crates.io index is unreachable but the dependencies are already in the
local Cargo cache, prefix the commands with `CARGO_NET_OFFLINE=true` (or pass
`--offline`).

The existing `.cargo/config.toml` provides the Espressif RISC-V C toolchain,
`LVGL_SYSROOT` and the `linkall.x` linker argument. The target runner is
`espflash flash --monitor --chip esp32s31`; do not invoke it without a board.

## Console path and command

The board's USB-C connector is **USB-OTG HS**, not USB-Serial-JTAG. The
firmware enumerates a CDC-ACM device on `USB_HS` and installs a custom
`log::Log` implementation. No second logger is installed, so `log::info!` /
`log::warn!` from every task reaches only the USB CDC port.

With the board connected over USB-C:

```bash
# Read-only monitor; auto-detects the USB CDC port.
espflash monitor --chip esp32s31

# Or select the USB-OTG HS CDC port explicitly. On macOS it is a
# /dev/cu.usbmodem* node (the exact suffix changes per enumeration).
espflash monitor --chip esp32s31 --port /dev/cu.usbmodem113401
```

List the candidates with:

```bash
ls /dev/cu.usbmodem*
```

The port appears only while the application is running; it disappears during
download mode and reappears after the app starts. A full flash+monitor run can
use the configured runner:

```bash
cargo run -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
```

The banner and task heartbeats no longer depend on `ESP_LOG`; the logger
accepts levels up to `Info` unconditionally.

## Flash command

Build the app image with the official `main_app` region as the target:

```bash
cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf
espflash save-image --chip esp32s31 \
  target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw \
  mosaico-s31-fw.bin
esptool --chip esp32s31 --port <PORT> --baud 115200 \
  --before no-reset --after hard-reset \
  write_flash 0x210000 mosaico-s31-fw.bin
```

Only `0x210000` is written. The bootloader, partition table and `vibe_mode`
at `0x20000` are left untouched.

## Bring-up history

R0 (console, heartbeats, full-frame transfer, touch logs), R1 (async QSPI,
GPIO6 input, LVGL `ui_task`) and G2 (ball first screen) were accepted on
hardware; the measurements are in the root README and the RFC 0005 ledger.
The R0/R1 full-screen presenter (`display_task`, `present`, `pointer_port`)
was removed once the fixed ball plane replaced it.

## Module provenance

Ported without register-sequence changes from
`archive/crates-pre-grok-ball-20261020/mosaico-s31`:

- `src/board.rs`
- `src/drivers/display.rs` + `src/drivers/display/packing.rs`
- `src/drivers/touch.rs` + `src/drivers/touch/protocol.rs`
- `src/drivers/shared_i2c.rs`
- `src/framebuffer.rs`
- `src/input_queue.rs`
- `src/memory.rs`

`lvgl_port.rs` started from the archived port and is now a headless LVGL
display registration.

## BallWidget and host preview (G1.2)

The crate now also exposes a `no_std` + `alloc` library target
(`mosaico_s31`) with the host-buildable `ball_widget` module. The ESP
dependencies stay under
`[target.'cfg(target_os = "none")'.dependencies]`; only `lvgl`, `lvgl-ui` and
`grok-ball` are normal dependencies.

`BallWidget` implements `lvgl_ui::CustomWidget`:

| Hook | Behavior |
| --- | --- |
| `create` | Leaks a two-slot `CanvasPool` of 240×240 RGB565 frames, creates `lv_canvas_create` as a direct child and publishes a black first frame. In plane mode the LVGL canvas is hidden and frames go to `ball_plane_task` via `BallControl::take_frame`. |
| `update` | Advances `BallEngine` by `elapsed_ms` on every tick, applies an expression change from `BallControl`, and rasterizes on its 50 ms cadence (or immediately after a touch) into a free pool slot. If no slot is free the frame stays due. In LVGL mode it swaps the canvas buffer and invalidates. |
| `delete` | Accounts the deletion. The pool is `'static`; slots are released when their leases drop. |

Application code keeps the mounted instance and changes it through a shared
handle, because a later `Node::custom` descriptor with the same key is
discarded by the reconciler:

```rust
let control = BallControl::new(BallExpression::Idle);
// App::view:
Node::custom(BallWidget::new(control.clone()))

// Anywhere on the UI task, without remounting:
control.set_expression(BallExpression::Thinking);
control.mark_dirty();
```

`BallControl` also reports lifecycle/observability counters used by the
preview and by future tests: `create_count`, `update_count`,
`rendered_frames`, `invalidations`, `delete_count` and `last_dirty_rect`.
`invalidations` equals `rendered_frames`, which proves the no-op update path
does not invalidate the canvas.

The preview registers a headless 480x480 RGB888 memory display (no SDL),
mounts one canvas, and cycles all 14 `BallExpression` values for 125 ticks of
16 ms each. It writes one PPM (P6) per state under
`target/ball-widget-preview/`, asserts that the widget is created exactly once
and deleted exactly once, checks that the canvas is 320x320 and centered, and
prints the counters:

```bash
cargo run -p mosaico-s31 --example preview --offline
```

The preview uses whatever `grok_ball::raster::render` currently provides. In
this workspace it produced the real body/eye polygon frames: the 14 PPMs have
different hashes and 40-80 colors each, and the ball is centered on the black
canvas field. If `render` is temporarily a placeholder that fills zeroes, the
same preview prints one color for the canvas region and identical hashes for
all states, which distinguishes a placeholder from real output. Either way the
preview proves the LVGL lifecycle, the canvas buffer wiring, the 30 fps
invalidation cadence and the shared-control path.

### Buffer/native-object drop order

`lvgl-ui` calls `CustomWidget::delete` while the native canvas is alive, then
drops the boxed widget (and with it the canvas buffer), and only then calls
`lv_obj_delete` on the canvas. This is safe because no LVGL render can run
between those steps: they execute synchronously inside one `Ui` method,
rendering only happens from `lv_timer_handler` / `lv_refr_now`, and the `Ui`
busy guard rejects reentrancy from a widget hook. LVGL's canvas destructor
tears down the image descriptor without reading the pixel buffer, so the
normal `Box` ownership is kept instead of leaking the buffer with
`ManuallyDrop`.
