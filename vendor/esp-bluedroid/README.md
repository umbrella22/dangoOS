# Bluedroid Classic audio host

This package builds Espressif's Bluedroid host and SBC decoder from ESP-IDF
commit `4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`. `UPSTREAM.json` records every
imported upstream path and SHA-256. Imported sources retain their licenses;
`LICENSE` contains the package's Apache-2.0 license. `src/` and `shim/` hold
the integration entry points. Changes to imported host files are listed with
their original and local hashes in `PATCHES.json` and reproduced by
`patches/mosaico.patch`.

`src/classic.c` exposes a Classic A2DP Sink with SBC decoding, SSP Just Works
pairing and AVRCP absolute volume. The advertised name is `ESP-Mosaico Audio`,
with an audio/rendering device class for operating-system pairing lists.
There is no BLE GATT host, A2DP source or HFP microphone service.

The host uses the firmware's existing esp-rtos scheduler. FreeRTOS-compatible
shim entry points forward to native tasks, queues, semaphores and timers in
`crates/os/src/platform/drivers/radio/classic/`. Waiting adapters yield without holding
a critical section. NVS records live on the NAND-backed data filesystem; host
file IO runs on native threads rather than the audio interrupt executor.
`src/crypto.c` provides the pinned controller's P-192/P-256, SHA-256 and HMAC
callbacks using the imported TinyCrypt implementation; known vectors and
invalid public keys are exercised by `tests/host/test_classic_crypto.py`.

One lifecycle worker owns blocking host startup and shutdown while the radio
task pumps HCI. Teardown retains the controller if host shutdown fails. PCM
callbacks only enqueue complete frames for the separate audio task. The
transport, lifecycle, codec and filesystem ownership boundaries remain local
so an upstream host/controller implementation can replace them without changing
the PCM service, settings or applications.

The shim enables `CONFIG_BT_BLUEDROID_ESP_COEX_VSC`. Upstream BTA sends A2DP
streaming start/stop through the Espressif coexistence HCI command so the
controller can coordinate airtime with Wi-Fi. Host and UI core affinity does
not provide that RF coordination.

The UI acknowledges a pause between resident Lua callbacks before native
controller/host initialization or teardown begins. The VM resumes with the same
state after the operation; its instruction and wall-time budgets remain active.
An unavailable UI prevents the native operation from starting.

The fixed queue publishes a newly appended item's dequeue credit before releasing
its list mutex. The HCI worker checks list length and reposts itself while items
remain; exposing an item before its credit lets that higher-priority worker
spin on an empty dequeue and starve the producer. The deterministic regression
executes the imported queue/list and HCI handler with producer preemption at
mutex release (`tests/host/test_classic_fixed_queue.py`).

AVRCP lifecycle calls return after enqueueing work. Startup and shutdown wait
for the corresponding profile acknowledgement; AVRCP CT/TG shutdown completes
before A2DP shutdown. A retained RTC phase records the last lifecycle operation
so a stalled vendor call can be identified after a manual reset.

Building and linking do not establish RF or audible playback correctness.
The source fingerprint, image hash and actual device results are recorded in
`target/audio-classic-20261028/receipt.json` at the workspace root. Any upstream
migration must repeat discovery, pairing, SBC playback, volume, stop/restart,
Wi-Fi coexistence and OTA checks before replacing this backend.
