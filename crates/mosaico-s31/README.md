# mosaico-s31 — ESP32-S31 firmware

Firmware and host-testable library for the ESP-Mosaico v1.2 fox-dango
companion. The library (`src/lib.rs`, `mosaico_s31`, `no_std + alloc`) holds
everything that does not touch hardware and runs `cargo test` on the host; the
firmware binary `mosaico-s31-fw` (`src/main.rs`) holds the drivers and the
Embassy tasks. The full tree is in
[docs/PROJECT_STRUCTURE.md](../../docs/PROJECT_STRUCTURE.md).

| Library module | Role |
| --- | --- |
| `behavior` | `Expression`, the affect engine (sensors → mood), the personality engine (taps), `Runtime` (merge + haptics) |
| `fox` | palette RLE sprites, `FoxAnimator` (expression + time → `Scene`), `FoxRenderer` (`Scene` → RGB565), entrance |
| `ui` | `MosaicoApp` screen tree, `FoxWidget`/`FoxControl`, two-slot `CanvasPool`, dormant splash |
| `telemetry` | touch-to-first-pixel latency and display counters |

## Tasks

`esp-rtos` + Embassy with two executors on core 0:

| Task | Executor | Behavior |
| --- | --- | --- |
| `plane_task` (`tasks/plane.rs`) | interrupt (FROM_CPU_INTR1, priority 2) | Sole owner of the CO5300. Takes the newest canvas-pool slot and uploads the 352×336 canvas at panel origin (64, 88) in 11 × 32-row async QSPI DMA chunks; animation frames wait for TE, touch-tagged frames skip it. |
| `ui_task` (`tasks/ui.rs`) | thread | Sole owner of LVGL. Every 5 ms: drain sensor events and touches into `behavior::Runtime`, advance the fox widget (render into a free pool slot on its 50 ms cadence, or at once after a touch), hand the frame to the plane, log stats. LVGL never renders the screen. |
| `input_task` (`tasks/input.rs`) | thread | Sole owner of the CST9220. GPIO6 edge + 5 ms fallback, async I2C reads into a bounded channel. |
| `sensor_task` (`tasks/sensors.rs`) | thread | BMI270, two BMM150, BQ27220, right-slot Interaction module; bounded `SensorEvent` channel. |
| `haptic_task` (`tasks/haptic.rs`) | thread | Motor patterns on GPIO8 via RMT-generated 5 kHz PWM. |

The console (`console.rs`) is a USB-OTG HS CDC-ACM device. `log` records are
formatted into fixed 160-byte records and queued through a bounded
`embassy_sync` channel; a pump writes them to the CDC port once the host opens
it. `esp-println` is not used for application logs: its `jtag-serial` path is
not wired to the USB-C connector on this board.

## Hardware truth

- CO5300 QSPI AMOLED, 480x480, RGB565, 40 MHz, SPI2 + AXI DMA.
- QSPI register opcode `0x02`, GRAM write opcode `0x32`; `CASET`/`RASET`
  windows are inclusive and x must stay 4-pixel aligned.
- CST9220 capacitive touch at 7-bit I2C address `0x5A` on I2C0
  (SDA=56, SCL=3, 400 kHz). INT is GPIO6 (R1).
- VCC_3V3 (GPIO60) is active-low and must be on before panel/touch init.
- Official partition: write only `main_app` at `0x210000`. Do not rewrite the
  bootloader, the partition table, or `vibe_mode` at `0x20000`.

## Dependencies

- `esp-hal`, `esp-alloc`, `esp-bootloader-esp-idf` and `esp-rtos` (features
  `esp32s31`, `embassy`, `log-04`) all come from the same pinned esp-hal git
  revision. The registry esp-rtos links a second `esp-rom-sys`, which Cargo
  rejects, so take all four from git and bump them together.
- `embassy-executor 0.10`, `embassy-time 0.5`, `embassy-usb 0.6`,
  `embassy-futures 0.1`, `embassy-sync 0.8`.
- ESP dependencies live under `[target.'cfg(target_os = "none")'.dependencies]`;
  only `lvgl`, `lvgl-ui` and `log` are normal dependencies, so the library
  builds on the host.

## Build, test, flash

```bash
cargo test -p mosaico-s31 --offline            # library
../../scripts/test-drivers.sh                  # pure driver modules (bin has test = false)
cargo run -p mosaico-s31 --example preview --offline
cargo fw --offline                             # release firmware
../../scripts/flash-app.sh <PORT>              # writes main_app at 0x210000 only
```

## Console

The board's USB-C connector is **USB-OTG HS**, not USB-Serial-JTAG. The
firmware enumerates a CDC-ACM device on `USB_HS` and installs a custom
`log::Log` implementation. No second logger is installed, so `log::info!` /
`log::warn!` from every task reaches only the USB CDC port.

With the board connected over USB-C:

```bash
# Read-only monitor; auto-detects the USB CDC port.
espflash monitor --chip esp32s31

# The running app enumerates as /dev/cu.usbmodemmosaico_s31_r11 on macOS.
espflash monitor --chip esp32s31 --port /dev/cu.usbmodemmosaico_s31_r11
```

If `espflash monitor` prints nothing, open the port with any serial tool
(pyserial works). The port exists only while the app runs; it disappears in
download mode.
