# mosaico-s31 — R0 platform skeleton

ESP-Mosaico v1.2 (ESP32-S31) firmware crate for the RFC 0005 platform
runtime. R0 runs one `esp-rtos` + Embassy executor with three tasks:

| Task | R0 behavior |
| --- | --- |
| `ui_task` | 1 s heartbeat log. LVGL is **not** initialized and must not be touched in R0. |
| `display_task` | Clears the full 480x480 framebuffer to black, draws a 480x120 three-band color bar (red/green/blue) on top, transfers the full frame through the archived CO5300 QSPI `write_chunk` path, logs the transfer time, then idles. |
| `input_task` | Polls the CST9220 every 5 ms; logs the first point of each touch and each release. GPIO6 interrupt is R1. |

The console is a USB-OTG HS CDC-ACM device. `log` records are formatted into
fixed 160-byte records and queued through a bounded
`embassy_sync::channel::Channel`; a pump writes them to the CDC port once the
host opens it. `esp-println` is no longer used for application logs: its
`jtag-serial` path is not wired to the USB-C connector on this board.
The crate also carries the archived (unused on the R0 hot path)
`framebuffer`, `input_queue` and `memory` modules so R1 can build on them
without re-porting.

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
local Cargo cache, prefix the commands with `CARGO_NET_OFFLINE=true`. The R0
build in this workspace used that switch because `index.crates.io` timed out;
`static.crates.io` and GitHub were reachable.

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

## Flash command (documented only — not run in this work package)

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

## R0 gate

Exit gate `P-R0` from RFC 0005:

1. `cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf`
   passes.
2. On the device, the console prints the banner and all three task heartbeats
   for at least 60 seconds without a watchdog reset.
3. The display task clears and transfers the full 480x480 frame (black field
   with the three-band color bar) and logs its transfer time.
4. A touch logs its first point and the release logs a second line.
5. No LVGL initialization is required or performed.

### Verified in this run (host/build only)

- `cargo check` and `cargo build --release` for
  `riscv32imafc-unknown-none-elf`.
- The pinned esp-hal/esp-rtos/Embassy dependency graph resolves to one esp-hal.
- The ported driver sources are present and the pin/panel/touch constants match
  `docs/MOSAICO_V12_HARDWARE.md`.
- `Usb::new_hs(peripherals.USB_HS)` and `USB_HS` compile for `esp32s31`.

### Still requires the board

- CDC-ACM enumeration as `/dev/cu.usbmodem*` while the app runs.
- Console banner, `ui_task` heartbeats, full-frame transfer time and
  touch/release lines over that CDC port.
- Full-frame `display_task` transfer time.
- Touch/release logs from the actual CST9220.
- 60-second no-watchdog run.

The target build is **not** device acceptance; `P-R0` remains open until the
board run is recorded in the RFC 0005 ledger.

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

R0 does not include the archived `lvgl_port.rs`, `pointer_port.rs`,
`touch_input.rs` or `ui.rs`; they belong to later R1/R2 work.

## BallWidget and host preview (G1.2)

The crate now also exposes a `no_std` + `alloc` library target
(`mosaico_s31`) with the host-buildable `ball_widget` module. The R0 binary is
unchanged and the ESP dependencies stay under
`[target.'cfg(target_os = "none")'.dependencies]`; only `lvgl`, `lvgl-ui` and
`grok-ball` are normal dependencies.

`BallWidget` implements `lvgl_ui::CustomWidget`:

| Hook | Behavior |
| --- | --- |
| `create` | Allocates `Box<[u16; 320 * 320]>`, creates `lv_canvas_create` as a direct child, calls `lv_canvas_set_buffer(..., LV_COLOR_FORMAT_RGB565)` and `lv_obj_center`. |
| `update` | Advances `BallEngine` by `elapsed_ms` on every tick, applies an expression change from `BallControl`, rasterizes when 33 ms elapsed or the control is dirty, then calls `CustomObject::invalidate()` only for a rendered frame. |
| `delete` | Accounts the deletion; the canvas buffer drops with the widget. |

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
