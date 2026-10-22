# ESP-Mosaico S31 — fox-dango companion

Rust firmware for the ESP-Mosaico v1.2 display terminal (ESP32-S31, CO5300
480×480 QSPI AMOLED, CST9220 touch, BMI270/BMM150, BQ27220). The screen shows
"fox-dango", a palette-sprite fox whose expression follows touch, motion,
light, magnets, the two expansion slots and the battery.

## Layout

One crate, `crates/mosaico-s31`:

- the **library** (`src/lib.rs`, `no_std + alloc`, host-testable):
  `behavior/` (affect + personality state machines), `fox/` (sprites,
  animation, renderer), `ui/` (LVGL tree, fox widget, canvas pool),
  `telemetry/`;
- the **firmware** (`src/main.rs`): `drivers/`, `tasks/` (Embassy tasks),
  `board.rs`, `console.rs`.

See [docs/PROJECT_STRUCTURE.md](docs/PROJECT_STRUCTURE.md) for the full tree
and [docs/FOX_RENDER_PIPELINE.md](docs/FOX_RENDER_PIPELINE.md) for how a frame
goes from PNG masters to the panel. The art and its Python pipeline live in
`assets/fox-dango/`. The UI layer is
[`lvgl-ui`](https://github.com/umbrella22/lvgl_binding_rust/blob/master/lvgl-ui/README.md) from
[lvgl_binding_rust](https://github.com/umbrella22/lvgl_binding_rust).

The earlier vector "grok-ball" core (`crates/grok-ball`) was removed after the
fox shipped; its behaviour logic moved into `behavior/`, the rest is in git
history (last present at commit `710d9dc`).

## Commands

```bash
cargo test -p mosaico-s31 --offline                          # library tests
./scripts/test-drivers.sh                                    # pure driver modules
cargo run -p mosaico-s31 --example preview --offline         # 16 expressions on a host LVGL display
cargo run -p mosaico-s31 --example splash_preview --offline  # splash + entrance frames (PPM)
cargo fw --offline                                           # firmware release build
./scripts/check.sh                                           # all of the above
```

## Current firmware behavior

- Touch goes to the personality engine (tap ladder: curious, puzzled,
  annoyed, angry), sensor events to the affect engine. `behavior::Runtime`
  merges them: urgent affect (Error, Panic) > touch reaction > affect
  expression > resting expression. Haptics follow the same order.
- Display: LVGL only hosts the widget lifecycle and never renders the screen.
  The fox widget renders a 352×336 canvas into a two-slot canvas pool on the
  UI task; `tasks::plane` uploads the newest slot over async QSPI (11 × 32-row
  DMA chunks, TE-synced) from a priority-2 interrupt executor, at panel
  origin (64, 88).
- Measured on device (image `target/mosaico-s31-fox2.bin`): render 10.1–15.0
  ms (p50 10.4), upload 13–14 ms, 18–20 fps, zero dropped frames.

## Build environment

- `lvgl` and `lvgl-ui` are Git dependencies on
  [lvgl_binding_rust](https://github.com/umbrella22/lvgl_binding_rust), pinned by `rev` in the root `Cargo.toml`. To
  try a local checkout, add a `[patch."https://github.com/umbrella22/lvgl_binding_rust"]` section in your own
  `.cargo/config.toml`; do not commit it.
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

Write only the `main_app` region (`0x210000`):

```bash
./scripts/flash-app.sh <PORT> [image-name]
```

Do **not** rewrite the bootloader, the partition table or `vibe_mode` at
`0x20000`. The official v0.5.0 layout keeps `main_app` at `0x210000`.

## Console

The console is a USB-OTG HS CDC-ACM device (`Usb::new_hs` + `embassy-usb`), not
USB-Serial-JTAG; the running app enumerates as
`/dev/cu.usbmodemmosaico_s31_r11`. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)
for the stats line.

## Documents

- [Source layout](docs/PROJECT_STRUCTURE.md)
- [Fox render pipeline](docs/FOX_RENDER_PIPELINE.md) (Chinese)
- [How the fox is drawn, and how to draw a new character](docs/FOX_DRAWING_GUIDE.md) (Chinese)
- [Development, checks, flashing](docs/DEVELOPMENT.md)
- [Mosaico v1.2 hardware notes](docs/MOSAICO_V12_HARDWARE.md)
- [Current state and remaining work](docs/plans/ROADMAP.md); RFCs in `docs/plans/`, archived plans and ledgers in `docs/legacy/plans/`
- [Legacy fan/Slint material](docs/legacy/README.md)
