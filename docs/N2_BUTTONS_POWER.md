# N2 — onboard buttons and power-off

## Hardware ownership and source constraints

`tasks::buttons` owns AI GPIO7, BOOT GPIO61 and PWR_SW GPIO57. AI and BOOT
are low-active inputs with pull-ups, sampled independently of the sensor/I2C
startup at a nominal 5 ms period. GPIO57 is configured as an **open-drain**
output, initially high (released, high impedance), with pad hold released.
Power-off asserts low and leaves it asserted; it never drives push-pull high.

The ESP-Mosaico BSP `include/bsp/esp_mosaico.h` defines GPIO57 and assert/release
levels 0/1 at lines 126–128, and AI7/BOOT61/active level 0 at lines 131–133.
`onboard/esp_mosaico.c` initializes the switch as released open-drain and clears
pad hold (`bsp_power_init`, lines 166–174). `bsp_power_set_shutdown` (lines
277–285) asserts low or releases high impedance; it specifies no pulse duration
or timed handshake. BOOT's application events do not change its ROM boot-mode
behavior: holding BOOT during reset can select the bootloader.

## Gesture and UI behavior

The host-testable `buttons::Button` state machine uses:

| Rule | Value / outcome |
| --- | --- |
| Debounce | 20 ms stable level on both edges |
| Single click | Confirm only after 180 ms without the next press |
| Double click | Two clicks in the same window; no preceding single |
| Dense third and later clicks | Coalesce to one native Double after the window; Lua retains its existing multiple-click behavior |
| Long press | Once at 1500 ms after debounced down; clears pending clicks |
| Release after long | No single or double emitted |
| Held during startup | Ignore gesture timing until the first stable release; no invented initial down time |
| Hold released before long threshold | Normal click classification; no shutdown request |

AI emits `ButtonPower`, BOOT emits `ButtonBoot`, with `ButtonShort`,
`ButtonDouble` or `ButtonLong` into the existing bounded sensor-event channel.
Home routes them into the affect engine. The existing engine maps AI short to
Listening and AI double to StopActivity; BOOT remains debug-only. Off Home,
the existing paused-fox routing and Settings light/haptic switches are
preserved. AI long has one explicit global exception: terminal shutdown works
from every page, independent of those switches or Lua ownership.

## Lua button interface and compatibility

`button.new(7, active_level, long_ms, short_ms)` and `button.new(61, ...)` now
open the onboard buttons. The existing `on`, `off`, `get_key_level`, `dispatch`
and `close` interface and callback event names apply unchanged. Defaults remain
low-active, 1500 ms long, 180 ms click window; caller thresholds remain local to
Lua and do not change the system shutdown threshold. Before the onboard task
has initialized, opening returns `nil, "onboard buttons unavailable"`.

The firmware appends an independent `OnboardButtons` structure to the internal
Rust/C hardware snapshot: ready flag, two pressed levels, wrapping sequence,
and a 16-entry timestamped edge ring. Lua receives this as a separate domain,
with no interaction-slot generation. GPIO11/10 or GPIO13/12 keep their original
availability, generation, queue and detach errors. Detaching/reinserting the
interaction board therefore invalidates its old handles while AI/BOOT handles
continue working. Both domains retain explicit queue-overflow errors; this is
not an unbounded event store.

Callbacks still run only when the application calls `button.dispatch`; no Rust
or interrupt context invokes Lua. Opening while held has no known down time,
so the release does not invent a click or long press. Like other page exits,
shutdown leaving a Lua page stops its VM: an undispatched final AI long callback
can be canceled by system shutdown. Lua callbacks are not shutdown vetoes and
are not guaranteed finalizers. An application that needs an earlier AI long
observation may choose a Lua long threshold below 1500 ms and dispatch
frequently; it cannot delay or intercept system shutdown. BOOT has no system
shutdown action.

## Sleep-to-power sequence

1. The first confirmed AI long press latches `Shutdown::WaitingForSleep`.
   Releasing the button, repeat long events or further clicks cannot cancel or
   restart it. Detection does not depend on successfully enqueueing the emotion
   event when the bounded channel is full.
2. The UI returns to Home, reconciles the fox widget and stops an active Lua
   page through the existing page lifecycle. `Runtime::begin_shutdown` is
   terminal: it publishes Sleep and a neutral pose, ignores later sensor/touch
   reactions, and returns no new runtime haptics. Queued navigation cannot
   replace the terminal Home page.
3. The normal fox entrance is allowed to finish. A plane job is tagged as
   shutdown Sleep only on a tick that actually rendered a new fox frame and
   after the entrance ended. Unrelated or old ready frames are not receipts.
4. The plane reports the job tag and the actual upload success into the pure
   shutdown state machine. Failed or untagged uploads cannot arm the timer.
   The first successful tagged upload begins a 1500 ms display interval;
   subsequent frames do not restart it.
5. After that interval the button task asserts GPIO57 low **once**, retaining
   low if external power prevents immediate loss of execution. If no successful
   Sleep upload arrives, it keeps GPIO57 released and logs a warning after
   5 seconds. A display stall does not silently bypass the animation contract.

An upload receipt proves the panel driver accepted all chunks, not optical
visibility. Real panel behavior, supply cutoff and wake circuitry require
physical acceptance. Existing haptic bursts already in progress are finite;
this sequence does not introduce a new motor waveform or power pulse.

## Validation and physical acceptance

The implementation is host-testable without GPIO access. Targeted checks:

```sh
cargo test -p mosaico-s31 --lib --offline buttons -- --nocapture
cargo test -p mosaico-s31 --lib --offline behavior::runtime::tests -- --nocapture
cargo test -p mosaico-lua --lib --offline tests_hardware -- --nocapture
cargo fw --offline
cargo fmt --all -- --check
git diff --check
```

The first command covers native debounce/bounce, delayed single, double,
canceled hold, held-at-boot suppression, repeated long suppression, dense
clicks, failed/unrelated frame receipts, and one-shot cutoff timing. Runtime
checks cover terminal Sleep and haptic suppression. Real C/Lua bridge tests
cover onboard short/double/long callbacks, held-open behavior, and independent
onboard handles across slot detach/reinsert alongside original N1 contracts.
Firmware compilation validates actual pin configuration, task spawning and the
UI/plane wiring; no hardware operation is part of these checks.

Pending physical checks:

- AI and BOOT short/double/long, bounce handling and boot-held release behavior;
  BOOT must not request power-off.
- GPIO7/61 Lua callbacks while both present and absent interaction boards;
  old slot handles must invalidate while onboard handles stay valid.
- AI short/double preserve Home emotion behavior; other pages and Settings
  preserve their prior light/haptic rules until terminal shutdown.
- AI long from Home, Settings and a Lua page: visible Sleep after any entrance,
  then one GPIO57 low assertion at least 1500 ms after the first Sleep upload.
- Confirm real supply current falls to the expected off-state, including USB
  and battery configurations; confirm AI can power the device back on.
- If a failed display upload can be induced safely, verify power remains on
  and the missing-frame warning appears; no unconditional cutoff fallback.

No physical button, panel, power-current, GPIO waveform or reboot acceptance
is claimed by host tests or a successful firmware build.

## 2026-10-23 acceptance receipt

The independent full `scripts/check.sh` run passed 306 host tests: 207
`mosaico-s31`, 47 `mosaico-lua`, 10 `mosaico-fs`, and 42 pure driver tests.
Both previews, release firmware, and the 627-symbol ROM collision check passed.
Logs and saved images are under `target/n2-acceptance-20261023/`; no image was
flashed during this stage.

| Artifact | Size / SHA-256 |
| --- | --- |
| `n2-default.bin` | 2,636,560 bytes; `d47de8381d923550edaaad16baaf38f14e51f0904feee116f9dfb5ecf02b7477` |
| `n2-default.elf` | `c3c49418cedeb5540ba524bf36251c62465cc02715f7102b7161ddf293f768be` |
| Changed implementation source manifest | `53f36956fd76bcef355071531d6fa6b7da4aeb143d041ab5a97c4e29f4d99886` |

`receipt.json` records each implementation file hash and the base commit. The
source digest hashes sorted `path + NUL + file_sha256 + LF` entries; documentation
is excluded. Later integration builds require their own receipts.
