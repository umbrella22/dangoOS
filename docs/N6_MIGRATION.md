# ESP32-S31 A/B layout migration and recovery

The migration tool prepares a concrete plan from a raw backup of this board.
It changes the partition-table sector, otadata, the application at 0x210000 and
the new data range. Existing data is preserved in the backup and is not migrated
into the smaller filesystem. The original firmware/layout/data can be restored
and checked byte for byte.

The tools and their offline simulations are implemented. Physical backup,
repartitioning, boot, rescue firmware and OTA acceptance are **not-run**.
Actual repartitioning follows the explicit-approval requirement in
[HANDOFF.md](HANDOFF.md#红线). `plan` is offline; `backup` is read-only on the
device. `apply` and `restore` require explicit confirmation switches and an
attended board already in download mode.

## Layout and protected bytes

| Partition | Offset | Size | Type/subtype |
| --- | --- | --- | --- |
| main_app | 0x210000 | 0x300000 | app / ota_0 |
| ota_1 | 0x510000 | 0x300000 | app / ota_1 |
| data | 0x810000 | 0x1D0000 | data / LittleFS |

The partition table is one sector at 0x8000; otadata is 0x9000..0xB000.
The migration verifies these entire protected ranges remain byte-identical:

- 0x0..0x8000, including the bootloader at 0x2000;
- 0xB000..0x210000, including NVS, PHY and the factory vibe_mode partition;
- 0x9E0000..0x1000000, including ui_apps and system.

The legacy source must positively match main_app at 0x210000 with size 0x7D0000,
the expected otadata and protected partitions, and no conflicting OTA/factory
layout. The raw table must have a valid MD5, valid bounds, unique names and no
overlap or encryption. The planner uses the pinned official ESP-IDF v5.5.1
generator, preserves every other partition record and verifies its round trip.
It does not substitute an upstream board's CSV. Generator provenance is in
[scripts/vendor/README.md](../scripts/vendor/README.md).

## Firmware compatibility

`scripts/ota_image.py` checks the complete ESP image: size within 3 MiB,
ESP32-S31 chip ID 32, 16 MiB/DIO/40 MHz flash declaration, segment lengths and
load ranges, no overlaps, entry point in a loaded segment, the first-segment
application descriptor for mosaico-s31, metadata strings, padding, XOR checksum,
embedded SHA-256 and exact end of file. Transport SHA-256 is a separate value.

Migration and OTA additionally require `MOSAICO_S31_AB_LAYOUT_V1\0` in the
image. This is a compatibility declaration, not authentication. Earlier N1–N5
firmware still derives data from the tail of main_app. Installing it under the
new table could format part of an application slot. A structurally valid old
image is therefore rejected before connecting or writing. `--structural-only`
allows offline inspection of older images and never authorizes installation.

Build and serialize the default N6 firmware before planning:

```sh
cargo fw --offline
espflash save-image --chip esp32s31 --flash-size 16mb --flash-mode dio --flash-freq 40mhz \
  target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw target/n6-application.bin
python3 scripts/ota_image.py target/n6-application.bin
esptool --chip esp32s31 image-info target/n6-application.bin
```

Structural validation does not prove the program will boot or pass self-test.
Keep the final build receipt and use the default firmware for migration, never
a deliberate-panic test image.

## Backup and offline plan

Use an esptool version with ESP32-S31 support. The following paths are examples;
the backup directory must be new and outside this repository. Both backup and
write commands leave the board in download mode.

```sh
python3 scripts/ota-layout.py backup --port /dev/cu.usbmodem113401 \
  --out /Users/ikaros/mosaico-backups/20261023-before-ab
python3 scripts/ota-layout.py inspect \
  --backup /Users/ikaros/mosaico-backups/20261023-before-ab
python3 scripts/ota-layout.py plan \
  --backup /Users/ikaros/mosaico-backups/20261023-before-ab \
  --image target/n6-application.bin \
  --out /Users/ikaros/mosaico-backups/20261023-before-ab/plan
```

`backup` reads all 16 MiB, independently rereads 0x0..0x20000 and requires those
bytes to match. It saves `flash.bin`, `boot-region.bin`, the original 2 MiB data
extract, device MAC, hashes, partition rows and both otadata entry CRC/state
observations in `backup.json`. Bootloader IDF/rollback strings are also recorded;
these strings cannot establish whether `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`
was enabled. That capability remains unknown until verified on the device.

`plan` creates `partitions.csv`, `partitions.bin`, a copied `application.bin` and
`plan.json` with the bound device identity, raw-backup hash, exact artifact
hashes and changed ranges. Review those artifacts before the attended write.
Any firmware change requires a new plan. The tool refuses to reuse an existing
plan directory.

## Attended application and recovery

After the concrete plan is approved, keep the board in download mode:

```sh
python3 scripts/ota-layout.py apply --port /dev/cu.usbmodem113401 \
  --backup /Users/ikaros/mosaico-backups/20261023-before-ab \
  --plan /Users/ikaros/mosaico-backups/20261023-before-ab/plan \
  --confirm-repartition
```

Before the first write, `apply` checks the same device MAC and reads all flash
again. It must exactly match the reviewed backup. The tool then writes the
compatible application, erases the new data range, writes the generated table
and erases otadata. A final full read verifies every protected byte, the exact
application/table bytes and erased data/otadata. The receipt says
`verified-write` and explicitly leaves boot acceptance unverified. Only then
reset for attended display, filesystem, input and rescue-firmware tests.

If interrupted or if the new layout cannot boot, return to download mode and
restore the original firmware, table, otadata and all old app data:

```sh
python3 scripts/ota-layout.py restore --port /dev/cu.usbmodem113401 \
  --backup /Users/ikaros/mosaico-backups/20261023-before-ab \
  --confirm-restore
```

Restore requires the same device, an intact original raw backup and unchanged
protected ranges. It writes the original main_app/data region, otadata and table
and requires the final 16 MiB read to match the original backup byte for byte.
It replaces all changes made inside those regions since the backup. It does not
attempt to repair damage to protected factory regions.

## Routine USB OTA

After successful migration and the baseline self-test, use the runtime console:

```sh
python3 scripts/push-app.py ota-status --port /dev/cu.usbmodemmosaico_s31_r11
python3 scripts/push-app.py ota target/n6-application.bin --port /dev/cu.usbmodemmosaico_s31_r11
# Wait for the board to reconnect and finish its self-test, then:
python3 scripts/push-app.py ota-status --port /dev/cu.usbmodemmosaico_s31_r11
```

The CLI validates locally before opening serial, requires a distinct inactive
target, sends ordered 4096-byte blocks with CRC, and checks each acknowledged
next offset. Partial serial writes are completed and zero progress is an error.
Disconnect/error aborts the transfer without retrying activation. Only the exact
`@ok ota-end` reply is reported as activated; it is not a boot or Valid receipt.
If that acknowledgement is lost, the CLI reports completion as unconfirmed and
requires inspecting status after reconnection. Normal OTA never writes the
partition table, data, bootloader or factory partitions.

Firmware ownership, self-test conditions, RTC fallback limits and detailed
protocol are specified in [N6_USB_OTA.md](N6_USB_OTA.md). Physical acceptance is
tracked in [N1_N7_DEVICE_ACCEPTANCE.md](N1_N7_DEVICE_ACCEPTANCE.md).
