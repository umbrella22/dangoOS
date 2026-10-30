This package is copied from esp-rs/esp-hal revision
`33f9b0f7c36ac47edbd015808b5399090fc1f9b6`. `UPSTREAM.json` records the
original file hashes. Sibling Cargo dependencies use that exact Git revision.

The S31 branch of `wr_btdm_osal_malloc` requests internal RAM. Its BLE
controller stores linked-list addresses in a hardware RAM window; allocating
these buffers from the general PSRAM heap triggered
`ble_lll_mmgmt.c:1987` with a PSRAM pointer `0x501b5c1c` that decoded as
`0x2f1b5c1c`. Other targets retain the upstream allocation path. Freeing uses
the existing allocator, which records the allocation size and locates its heap
region. No callback is replaced with fake success.

The local BR/EDR backend uses the matching controller configuration, baseband
and crypto callback tables, and Classic HCI packet ownership. The crypto
implementation comes from the separately pinned `esp-bluedroid` package.
`ble/btdm2/classic/pool.rs` keeps a small linked-list allocator and every
allocation inside the controller's 3 KiB pool. A global radio mutex serializes
pool operations. Backing alignment follows `Heap`'s actual requirement (four
bytes on RV32); returned payloads are eight-byte aligned. A multi-region
`EspHeap` cannot fit its metadata inside this pool. Host tests exercise the real
adapter for bounds, exhaustion, contents and coalescing.

The vendor assertion entry point is bridged into the firmware's retained panic
record. An assertion remains fatal; its file, line and arguments survive reset.
See `esp-wifi-sys-esp32s31/MOSAICO.md` for the archive-copy symbol change.

The firmware's HAL adapter and service contracts remain separate from this
patch. BLE GATT was used in earlier device acceptance and is no longer a
firmware service. A migration removes the `esp-radio` Cargo patch only after an
upstream implementation matches Classic discovery, SSP pairing, SBC reception,
PCM delivery, shutdown/restart, Wi-Fi coexistence, USB OTA quiescence and NAND/UI
regression with PSRAM enabled. Device results are recorded separately from
build success in `target/audio-classic-20261028/receipt.json`. The input S31
archives remain in the separately pinned `esp-wifi-sys-esp32s31` package.
