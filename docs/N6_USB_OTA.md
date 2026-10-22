# ESP32-S31 USB OTA

The USB CDC updater writes only the inactive 3 MiB application slot and the two
OTA selection sectors. It requires the verified A/B partition layout described
in [N6 migration](N6_MIGRATION.md). Legacy firmware keeps its positively identified
`main_app` tail filesystem; an unknown table never selects a writable filesystem
region and cannot start OTA.

## Wire protocol

Each command line starts with `@mosaico ` and ends with a newline. The host waits
for the final reply before sending the next command. Diagnostic lines may precede
it. Replies use `@ok` or `@err`.

| Command | Final success reply | Meaning |
| --- | --- | --- |
| `ota-status` | `@ok ota-status ...` | Layout, slot/transfer and actual health state |
| `ota-begin <bytes> <sha256>` | `@ok ota-begin <booted> <target>` | Quiescence completed; slot IDs are 0 or 1 |
| `ota-chunk <offset> <length> <crc32>` followed immediately by raw body | `@ok ota-chunk <next_offset>` | Sector erase, write and byte comparison completed |
| `ota-end` | `@ok ota-end` | Readback image verified and selection persisted; reboot follows confirmed endpoint transmission |
| `ota-abort` | `@ok ota-abort` | Incomplete transfer discarded and ordinary execution resumed |

SHA256 is 64 hexadecimal characters and CRC32 is eight hexadecimal characters.
Offsets start at zero and increase sequentially. Each body is 4096 bytes except
the final exact remainder. There is no intermediate body-ready acknowledgement.
At most one sector of incoming body is buffered. Finite malformed chunk headers
with a known length of 1..4096 cancel the transfer immediately, then consume the body before reporting failure;
these bytes cannot become commands. An unknown or oversized body length poisons
the parser until reconnect. Oversized or invalid UTF-8 control lines also cancel
the transfer and poison framing: a discarded header may hide an unknown raw-body
length, so following bytes cannot safely become commands. Disconnect abandons an unactivated transfer. Abort
after activation does not undo committed selection.

For `ota-end`, a dedicated queue fence is marked sent only after the CDC endpoint
successfully accepts the complete ACK record. Automatic reset requires that fence;
queueing alone is insufficient. Queueing plus transmission has a three-second
budget. Timeout withholds automatic reset, restores ordinary execution and retains
the committed candidate selection; the operator must inspect status and reboot.
An ACK still queued at timeout may subsequently reach the host, but does not
trigger a late reset. Endpoint completion does not prove the host process parsed
the line. Normal logs and other command replies keep their existing behavior.

The runtime refuses begin until this boot has completed selftest and the running
slot is `Valid`. The core independently refuses an unconfirmed running slot.
This prevents a newly booted, unconfirmed candidate from overwriting the last
known valid rollback image. Status remains readable during startup. On a validated A/B layout it emits two
`@ota-slot` diagnostic lines before the final `@ok ota-status` line. Each slot
reports its latest CRC-valid metadata state (`Unknown`, `New`, `PendingVerify`,
`Valid`, `Invalid`, `Aborted`) and a bounded first-segment descriptor read. An
absent/corrupt/unreadable descriptor reports `empty`/`invalid`/`unknown` instead
of printing untrusted bytes. `image=descriptor` establishes descriptor validity
only; status does not hash the complete image. All three lines fit 158 bytes.

## Write and verification boundaries

A shared atomic flash guard serializes all ROM calls and the internal-RAM bounce
buffer across VFS and OTA. PSRAM-backed callers never pass their buffers directly
to cache-disabled ROM routines. OTA work runs outside VFS locks. The runtime
quiesces the foreground Lua task, releases its native UI, pauses resident UI and
brain callbacks, discards queued input/sensor events, holds lights off and disables
haptics for the transfer. Each bounded chunk and readback step yields to USB.

Chunk acknowledgement follows erase, aligned write and byte-for-byte readback.
End performs a separate streaming SHA256 readback of the complete image and checks
ESP magic/chip/flash settings, bounded and non-overlapping segments, an entry in a
loaded segment, the first-segment application descriptor, zero padding, ESP XOR
checksum and embedded SHA256. Application project must be `mosaico-s31`; version
must be nonempty printable ASCII with canonical NUL termination. No trailing data
is accepted.

Activation also requires `MOSAICO_S31_AB_LAYOUT_V1` plus its terminating NUL in the
image prefix. This declares the new storage layout behavior: old firmware could
format part of a slot as legacy data after migration. The marker, hashes and CRCs
provide compatibility and corruption checks, not authentication or a signature.
The structural verifier exposes marker presence separately so older valid images
can be inspected without accepting them for activation.

OTA metadata follows the pinned ESP-IDF entry representation, sequence parity and
CRC convention. Selection updates the alternate sector while retaining the current
bootable entry. A torn invalid copy is tolerated only when another valid copy
survives. Readback failure can occur after bytes are already persisted; failure
does not prove selection remained unchanged. Initial blank metadata is established
as slot-zero `Valid` by successful selftest before the first transfer is allowed.

## Confirmation and warm-reset fallback

Confirmation requires both filesystems actually mounted, a real first display
frame, a successfully exercised Lua VM, UI heartbeats and at least 30 seconds of
continuous observed health; a loop gap greater than one second restarts the window. Merely finishing storage initialization is insufficient. The
running slot is identified using the pinned S31 MMU entry-zero mapping and must
match selected metadata before confirmation or activation.

After activation, a checksummed persistent RTC record identifies candidate slot,
previous slot and selection sequence. Its attempt counter increments at the start
of firmware main, before HAL, heap, PSRAM, display or VFS initialization. The fallback
reads flash after HAL initialization and validates the complete A/B partition table
without heap allocation. If the same unconfirmed candidate reaches its third warm
boot attempt, it selects the previous slot `Valid` and resets immediately. The
first two candidate boots may fail; the third main entry performs fallback rather
than attempting initialization again. Successful selftest
confirms the candidate and disarms the record.

This is an application RTC warm-reset fallback. It is not a claim of bootloader
rollback or cold-power-loss rollback. Execution before the main recorder, failure
inside HAL initialization, loss of RTC retention and inability to read/write flash
are outside the fallback boundary. Foreground Lua finalization is cooperative;
a synchronous native finalizer that never returns can prevent the executor from
observing the quiescence timeout. No flash update starts before quiescence succeeds.

The default-off Cargo feature `ota-test-panic` panics before PSRAM/display/VFS only
for a valid RTC-armed candidate matching `New`/`PendingVerify` selection. It does
not inject a panic on legacy, blank, already `Valid` or unrelated boots. Its offline
build proves compilation only; actual warm reset/fallback behavior needs a board.

## Build and recovery

Build with `cargo fw --offline`. Serialize images explicitly with:

```sh
espflash save-image --chip esp32s31 --flash-size 16mb --flash-mode dio --flash-freq 40mhz \
  target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw target/application.bin
python3 scripts/ota_image.py target/application.bin
```

Normal updates use the host OTA command documented by `scripts/push-app.py --help`.
`scripts/flash-app.sh` refuses its former unconditional write behavior. Explicit
`--rom-recovery-slot0` permits attended ROM recovery at `0x210000`, validates the
image and enforces the 3 MiB bound. It does not modify partition tables, bootloader,
protected partitions or OTA selection; a selected slot one may continue booting.
Use migration backup/restore tooling when recovery requires selection or layout
changes.

Host NOR tests cover activation, readback corruption, malformed binary framing,
unconfirmed-slot refusal, torn alternate metadata and each metadata I/O failure
point. Offline builds and host tests do not establish USB throughput, real flash
fault tolerance, physical motor/light quietness or warm/cold reset behavior. Those
hardware checks remain not run without an attached device.

## Scope retained from the N6 plan

The built-in fox assets remain compiled into the image. Asset removal is not
needed to fit the current 3 MiB slot and would remove the filesystem-independent
fallback; moving all assets out remains a separate N3/resource decision. Image
size is checked for each build rather than assuming permanent headroom.

The “within three resets” policy counts candidate main entries: after two failed
candidate boots, the third main entry performs fallback before PSRAM/display/VFS.
With `ota-test-panic`, the first two candidate boots panic and the third takes the
fallback path. This policy still requires hardware acceptance. There has been no physical bootloader rollback-option
inspection, factory rescue boot, ten-update persistence run or panic/reset run.
Wireless transport and an HTTP source adapter are deferred with Wi-Fi support;
the transport-independent Flash/updater core is shared by the USB integration.

Migration recovery requires the external intact backup and matching hardware
identity described in the migration guide. An interrupted migration may require
ROM download mode and attended restore. The runtime cannot repair a half-written
partition table, restore overwritten legacy data or automatically enter the
factory rescue application. The explicit slot-zero recovery script also leaves
OTA selection untouched, so it alone does not guarantee slot-zero boot.

## 2026-10-23 independent acceptance receipt

Base: `df6b6b4`. The source manifest, artifact hashes and log hashes are in
`target/n6-acceptance-20261023/receipt.json`. Implementation manifest SHA-256:
`f60525237c16c5d11a766248aea2ce6e48f3727ef7c01a784d2afd853e75592f`.
Documentation is excluded from that source manifest so the receipt can describe
the tested implementation without changing its fingerprint.

The final `scripts/check.sh` gate passed **360 tests** (255 S31, 53 Lua, 10 FS,
42 pure drivers), both existing previews, default release firmware, and the ROM
collision check over **713 C symbols**. The Python suite passed **46 tests**.
The default-off panic feature also compiled successfully; the normal ELF was
restored afterward and matched the archived default ELF byte for byte.

Independent probes used the actual default application image and a table from
the pinned official generator. They ran in separate processes with external
timeouts:

- `n6_image_probe` passed 24 final-image cases and 15 previously frozen cases,
  each streamed in fragments of 1, 17 and 4096 bytes. Cases cover header/chip/
  flash configuration, segment bounds/overlap, entry point, descriptor metadata,
  XOR, embedded hash, truncation, trailing payload and compatibility marker.
  The independent Python validator agreed on all 39 cases. Structurally valid
  old firmware remains inspectable but cannot pass the OTA activation gate.
- `n6_update_probe` ran ten alternating full-image updates, confirmation after
  each, and byte comparisons protecting the running slot, data and factory
  regions. Six rejection scenarios retained the old Valid selection: incomplete
  upload, CRC, offset, transport hash, readback corruption and a real legacy image.
  Thirty-nine before/partial/after I/O failure cases covered chunk operations,
  alternate metadata activation and confirmation; the previous Valid metadata
  copy survived. A failure after persistence may still leave a selected candidate;
  the probe does not assume failure means no bytes changed.
- Migration simulations used the real generator, bound backup/plan checks,
  exact apply/restore comparisons and interruption at each migration write step.
  USB tests covered partial writes, binary command-looking data, offsets, local
  rejection before connecting, lost end acknowledgement and visible slot status.
  These synthetic flash images are host fixtures, not artifacts for a board.

Default firmware: **3045376 bytes**, leaving **100352 bytes** in a 3 MiB slot.
Serialization explicitly selected 16 MiB / DIO / 40 MHz. esptool independently
accepted chip 32, checksum and embedded hash; the Python validator confirmed the
A/B marker. SHA-256:

```text
BIN 17b1949ef90c1cd72de4f8fe7d6b1330e91c1d1de9390ed6c693883c0f9b6ab4
ELF fbb65ea000ba88645a6ecb089b9a7a3991d62578e5401d08134265087c46aa18
```

The files are `n6-default.bin` and `n6-default.elf` in the receipt directory.
The separately named `n6-panic-candidate.bin` is **3045472 bytes**, SHA-256
`153d37e409bdbb45f664d5d22f3745ac03dbbf204415db2c0b7ae66db1f3ac7e`.
It is reserved for attended candidate-reset testing and must not be used as
the initial migration image. The table and mutated image corpora in that
directory are simulation inputs; actual migration requires a new plan derived
from the device's verified backup.

Reproduction entry points are `scripts/check.sh`, Python unittest discovery
under `scripts/tests`, `scripts/n6-image-corpus.py`, and the Cargo examples
`n6_image_probe` / `n6_update_probe`. The latter takes a compatible BIN, generated
partition table and a legacy BIN; all three are files and no serial is opened.

No N6 device operation ran. Real USB transfer, flash timing, reset/rollback,
bootloader configuration, factory vibe_mode, physical output quiescence and
ten-update data persistence remain **not-run**. Earlier N1 motor failure and
stability observations remain open in the consolidated hardware checklist.
