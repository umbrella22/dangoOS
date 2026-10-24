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

At the original 2026-10-23 host acceptance snapshot, device/USB operation, physical flash interruption, actual panel busy-frame visibility, manual vibration and power-loss cycling were **not run**; updated device scope appears below. RAM LittleFS recovery tests establish logical intent semantics, not exhaustive physical power-cut coverage. The parent integration review owns full-suite, ROM-size and stage acceptance.

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


## 2026-10-24 device acceptance: partial pass through capture49

The ordinary firmware from `14316ff` (BIN SHA-256 prefix `7b7b65b0`) was tested with temporary downloaded IDs
`n7_probe_a_20261024` and `n7_probe_b_20261024`. Both temporary apps and the unique
legacy sentinel have completed cleanup. Capture 49 lists exactly hardware_probe,
hello_push, n1_contract_probe and n1_lifecycle_probe, skin default, brain default
CRC `bc14ae2e`, error=0, and 465/512 free data blocks. N7 remains partially accepted.
No production changes, repartitioning or physical interruption were performed.

| Accepted device case | Evidence and boundary |
| --- | --- |
| Independent roots and persistent same-name counts | 03–07 use `shared.count` under distinct VM roots; peer values remain correct. Uploading B rescans the registry and restarts foreground A, so counts follow actual STARTs. Sequential foreground VMs, not acceptance of concurrent VMs on the device or a security sandbox. |
| UI removal and fresh reinstall | 08 was an accidental confirmation; 09/10 directly establish A body/data absence and B preservation; 13 reinstalls A with count=1 and fresh=true while retaining B count=3. |
| Release, button/background/Back cancellation and haptic | 14/23/26 operator observations plus no new START and retained app listings. Capture 23 records MagneticContact, one 30 ms burst at 35%, and felt vibration. Single trials, no waveform or precise input latency claim. |
| Native and factory source UI | 25: Settings/Sensors/Timer no destructive long-press dialog/haptic; 2048/Album clear-data/keep-factory copy, cancelled. Four haptic entries cannot be mapped individually to source categories; category observation comes from the operator. No factory data was actually cleared. |
| Normal USB stop barrier | 17/19 raw/log/events agree: stopped→post-drop(vm) heap summary→rm/rmdata ACK. Later direct peer checks establish deletion/clearing. Quiet alone is not closure evidence; host queue arrival does not measure physical stop latency. |
| rmdata retains body and recreates fresh data | 21 sees B body=true/data=false before B restarts; 22 starts B at count=1 with fresh=true and retains A count=1. |
| Registry reorder retains confirmation identity | 27 retains A confirmation; 29 changes only B order from 98 to 0 and scans B before A; 30 confirms visible movement and no wrong deletion; 31/32 verify only A was removed and B data was retained. Origin-change rejection is a separate untested device case. Later AppStore launch was explicitly confirmed as an operator tap. |
| One unique legacy root sentinel survives removal/clear | 37 creates `/data/n7_legacy_guard_20261024_6d3f98a7.txt` only when absent; 41/45 verify exact fixed bytes after B rmdata/rm. No preexisting root files were scanned or modified. |
| Final cleanup | 45 directly stats B body/data absent; 48 checks exact owner and sentinel bytes, removes sentinel, asserts exists=false. The rm A ACK in 49 and exact original-four listing complete body cleanup; final A appdata deletion has management ACK evidence only, without an independent final stat. |

The sentinel contains 19 bytes and its A owner marker contains 29 bytes; four manifests share
one script and use global args through Vm::set_args_json. Each upload is at most
1,600 bytes. The sequence is create → owned B rmdata → check-clear → owned B rm →
check-remove → cleanup sentinel → rm A. Each mode upload follows a captured stop/known
text state; each subsequent stage binds the previous successful probe and matching
B-operation receipt. The through49 supplement verifies all 15 added phase file
fingerprints and those bindings. Preparation and execution records remain separate.

### Evidence ledger

All paths are under `target/n7-live-20261024/`. Earlier receipts and operator
snapshots are immutable; each supplement adds only its measured cases.

| Receipt | Scope |
| --- | --- |
| `partial-acceptance-through-13.json` | Initial data isolation, UI removal and reinstall |
| `partial-acceptance-through-14-supplement.json` | Button cancellation and target name |
| `partial-acceptance-through-22-supplement.json` | USB stop, rmdata and fresh recreation |
| `partial-acceptance-through-24-supplement.json` | Release/background cancellation and haptic |
| `partial-acceptance-through-25-supplement.json` | Native/factory dialog categories |
| `partial-acceptance-through-32-supplement.json` | Back, actual registry reorder, intentional AppStore tap |
| `partial-acceptance-through-49-supplement.json` | Legacy sentinel preservation/removal and final cleanup |

### Remaining device acceptance

Source-Origin changes, downloaded factory shadows, concurrent upload/delete,
stalled finalizers, physical interrupted intent/reboot recovery, visible busy/toast
and stopped-page timing, scroll/press-lost cancellation and repeated stability
remain unaccepted. Existing host tests cover several logical cases but do not
replace these device gates. Absolute io paths remain unrestricted. The single new
sentinel establishes one controlled preservation case, not a general audit of old
/data contents. A final independent appdata stat is absent for the last removed A.
