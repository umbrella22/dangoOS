# Lua app lifecycle and removal (N7)

Each firmware VM creates `/data/appdata/<id>/` and owns its validated, NUL-terminated storage root. `storage.get_root_dir()` reads the active VM context, including calls from Lua finalizers during `Vm::drop`. Interleaved VMs retain independent roots. The compatibility `set_data_root` function changes the default copied into subsequently created VMs; it now returns validation errors for NUL, relative or overlong paths. Existing VMs are unaffected. Default updates and constructor snapshots use one short critical-section lock; the C no-context fallback is immutable `/data`, so no mutable default pointer escapes. This storage convention does not sandbox absolute Lua `io` paths. Unattributed legacy files directly under `/data` remain untouched.

## Launcher

A short click opens the captured app ID after resolving its current registry index and verifying its source. A long press opens a confirmation dialog. Release after a long press cannot launch the app. Native Settings, Sensors and Timer tiles have no destructive long action. The dialog bounds the display name to one line independently of the complete ID, including embedded control characters; its confirmation and cancellation controls remain on screen for long names. The dialog names the app and distinguishes downloaded-app removal from factory-app data clearing. A downloaded app may shadow a factory app with the same ID; removal reveals that factory copy.

Confirmation retains exact ID and expected `Origin`, never a mutable tile index. Registry reorder preserves the operation. A changed source requires new confirmation, and the executor checks the source again immediately before filesystem mutation. Background click, Back, PressLost and ScrollBegin cancel confirmation; repeated confirmation cannot enqueue a second operation. While executing, ordinary UI navigation is suppressed. Terminal power shutdown remains higher priority.

The UI retains the operation until the ordinary LVGL busy-state screen has been submitted to the display plane and its `ScreenLease` has been released. Transition frames do not satisfy this gate. The UI then performs the stop barrier and filesystem operation, and shows a success or error toast. The toast remains on launcher visits until the next valid long-press action replaces it. A valid Lua long press emits one queued lightweight `MagneticContact` haptic request through the UI task, respecting the haptics setting; native tiles and repeated frames do not request a buzz. These are source-level device-path guarantees; physical panel timing is not yet verified.

## Execution and USB replies

The shared `hub::uninstall` API separates `Remove(Data)` from `Clear(Data/System/Builtin)`. `Remove(System/Builtin)` fails explicitly. USB uses `rm <id>` for downloaded-app removal and `rmdata <id>` for data clearing. Both wait for actual completion before producing `@ok`; filesystem failures produce `@err`. `scripts/push-app.py` supports both commands.

Management has one bounded owner. Its phases are `Idle -> Executing -> AwaitingRefresh -> Idle`. A competing owner receives a busy error. The management completion boundary cannot be overwritten by ordinary latest-navigation-wins requests: the UI must consume Stop plus registry rescan before the start barrier is released. Upload and run commands received during management are explicitly rejected as busy. A USB disconnect does not cancel an operation already owned by the executor.

The Lua task closes its start gate, clears queued starts, requests interrupt and waits until execution state is no longer running. That state changes only after `lua_close` and finalizers have returned. A locally captured next start is also checked against the gate. No VFS lock spans this asynchronous wait. Filesystem execution then runs synchronously with exclusive VFS access.

The console feeds whole raw USB payload chunks, stopping only at a deferred management-command boundary. It awaits the effect before feeding the remainder of the same packet. This preserves write batching for image/skin uploads and prevents a pipelined commit from running before removal. Same-ID staging is invalidated in protocol state for USB removal and removed on disk for both UI and USB removal. Each remaining partial-payload chunk checks the original path before writing; removed staging cannot be recreated by stale file or commit commands.

## Durable intent and recovery

For downloaded removal, the body is atomically renamed into `/data/apps/.trash/<id>`. This is the durable removal intent. Execution removes same-ID `.incoming`, removes `/data/appdata/<id>`, then removes the trash tree. Factory or explicit data clearing uses `/data/apps/.clear/<id>` as a distinct intent, preserving the body. Missing data directories are successful no-ops; other errors retain the intent and return failure.

Storage initialization recovers both intent directories before installing the VFS, and boot app scanning also retries recovery. Recovery includes appdata and incoming cleanup and is idempotent. Pending identities remain blocked at resolution, upload begin/commit, and VM start even if recovery fails and another registry scan occurs. Recovery never infers ownership of legacy `/data` files. No flash repartitioning or migration occurs.

## Evidence

Receipts are saved under `target/n7-acceptance-20261023/`. The production host VM test interleaves two VMs and checks roots from finalizers after dropping them in reverse order. Shared filesystem tests cover shadow removal, factory protection, explicit data clearing, interrupted trash/clear intent recovery, source-change rejection and legacy/unrelated-data preservation. Protocol tests cover partial payload followed by UI removal, stale commit failure, same-packet deferred boundaries, busy replies and 64/512-byte payload write batching. The pure ownership phase test requires the stop/rescan acknowledgement before accepting a subsequent owner.

`cargo run -p mosaico-s31 --example n7_lifecycle_preview --offline` runs the real application on an LVGL memory display with a real LVGL pointer input device. It holds beyond the long-press threshold, releases without launch, short-clicks to launch, opens Builtin/System/Data dialogs, cancels via background and Back, reorders registry entries before confirmation, captures the busy screen and success toast, and rejects a source change. PPM screenshots are under `target/n7-acceptance-20261023/ui/`.

Device/USB hardware operation, physical flash interruption, actual panel busy-frame visibility, manual vibration and power-loss cycling remain **not run**. RAM LittleFS recovery tests establish logical intent semantics, not exhaustive physical power-cut coverage. The parent integration review owns full-suite, ROM-size and stage acceptance.

## 2026-10-23 independent acceptance receipt

The parent review ran the full `scripts/check.sh` on the frozen N7 sources:
328 tests passed (227 firmware library, 49 Lua, 10 filesystem, 42 pure-driver),
both standard previews and the release build passed, and 626 C symbols had no
ROM collision. Independent Python discovery passed 17 tests. The parent also
ran the real LVGL pointer preview (8 scenarios) and inspected the dialog, busy,
completion, source-change and long-name renders. Formatting and diff checks passed.

- Review base: `2cb36dd2dc91cd80e212a7abf5fcdd29319cfeea`.
- Changed implementation-file digest: `7a8bba3b01801f18ee676fa26bff4eb44a832fe1721b3100089184f6ac29773e`.
- Saved ELF: `target/n7-acceptance-20261023/n7-default.elf`, SHA-256 `7f69173e0700799a61de9e534af42357a2a28893ee395662d5aa0b56d8ffeda5`.
- Saved BIN: `target/n7-acceptance-20261023/n7-default.bin`, 2,705,520 bytes, SHA-256 `3bd1c40e0c904a86fe818689f85cb8cd81ab0ee7d17e083800f8ee6bc6796494`.
- Planned 3 MiB OTA slot margin at this stage: 440,208 bytes.
- Detailed file hashes and counts: `target/n7-acceptance-20261023/receipt.json`; independent logs are `full-host-check.log`, `independent-python.log`, and `independent-ui.log`.

The regenerated Chinese hub font subsets include the lifecycle dialog/toast
characters. Existing underlying font family and display geometry remain the
same. Management currently quiesces the foreground Lua VM for either removal
or clearing; it does not preserve an unrelated foreground app during a USB
management operation. Hardware behavior remains deferred to the unified checklist.
