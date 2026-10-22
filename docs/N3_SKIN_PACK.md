# N3 — versioned fox skin packages

## Package and commands

The built-in fox remains the default and needs no filesystem. Optional skins
live at `/data/skins/<id>/skin.json` and `fox.bin`. IDs are exact identities:
1–63 ASCII letters, digits, `_` or `-`, with `default` reserved for the compiled
fallback. No ID is trimmed, sanitized or renamed to fit a path.

The existing art packer can write a package without modifying the compiled
`src/assets/fox.bin` or generated `src/fox/data.rs`:

```sh
python3 assets/fox-dango/tools/fox_pack.py \
  --skin-dir target/my-skin --id my_skin --name 'My skin' --version 1
# Deterministic recolor for acceptance; XOR applies to the RGB565 palette.
python3 assets/fox-dango/tools/fox_pack.py \
  --skin-dir target/recolor --id recolor --palette-xor 0x7800
```

`--device` selects a prepared art directory containing `manifest.json` and its
PNG assets. The original packer invocation without `--skin-dir` still generates
the firmware assets. Existing NumPy/Pillow dependencies apply. The pose keys
must match all 20 canonical names in their documented order; missing/reordered
poses are rejected rather than assigned a different expression.

On a connected runtime console, the existing push tool adds:

```sh
python3 scripts/push-app.py skin-push target/recolor --select --port PORT
python3 scripts/push-app.py skin-ls --port PORT
python3 scripts/push-app.py skin-select recolor --port PORT
python3 scripts/push-app.py skin-select default --port PORT
python3 scripts/push-app.py skin-rm recolor --port PORT
```

`skin-push` alone installs without changing the selected ID. Replacing an
already selected ID refreshes it immediately even without `--select`. `--select`
selects after installation. These commands do not flash firmware. Skin uploads
require exactly the two package files and reuse the existing `file` size/CRC
transport and safe path rules. App commands retain their existing syntax and
storage roots.

## Binary format version 1

All numbers are little-endian; unaligned reads are copied, never cast to a
native struct. The fixed 36-byte header is:

| Offset | Type | Field |
| --- | --- | --- |
| 0 | 8 bytes | `MSKIN01\0` |
| 8 | u32 | Binary schema version, 1 |
| 12 / 14 | u16 / u16 | Canvas width / height, 352 / 336 |
| 16 / 18 | u16 / u16 | Palette count (1–255) / pose count (20) |
| 20 / 24 | u32 / u32 | Metadata bytes / RLE blob bytes |
| 28 | u32 | zlib CRC32 of metadata plus blob |
| 32 | u32 | Reserved, must be zero |

Metadata starts with palette entries (`u16` RGB565), then 20 ordered pose
records. Each pose has its `u16` canonical ID, one base image record and eleven
`u16 count` + image-record arrays in this order: blink, mouth_closed,
mouth_wide, ear, gaze_left, gaze_right, symbols, tremble, spiral1, spiral2,
spiral3. An image record is 20 bytes: `i16 x, i16 y, u16 width, u16 height,
u32 offset, u32 length, u32 over_bg`. `over_bg` must be 0 or 1; offsets are
relative to the start of the following RLE blob. Row RLE pairs are `(u8 count,
u8 palette_index)`; counts are nonzero, and 255 is transparency for parts only.

`skin.json` has exactly these fields:

```json
{
  "schema_version": 1,
  "id": "recolor",
  "name": "Fox skin",
  "version": "1",
  "canvas": [352, 336],
  "binary_bytes": 742264,
  "binary_crc32": 0
}
```

The last two example numbers must be replaced with the actual whole-binary
length and zlib CRC32; the packer calculates both. Display name is a nonempty
string up to 96 UTF-8 bytes, version up to 32 bytes, with no control characters.
Schema and binary versions must both be 1. Duplicate, missing or unknown
manifest fields, identity mismatch and canvas mismatch are rejected.

The production parser checks total file length (at most 1 MiB), header fields,
CRC, metadata end, all ordered pose IDs, image offsets/lengths, dimensions,
canvas containment, flags, palette references, row run lengths and exact image
completion. Every base is an opaque full-canvas image. Layer capacity is checked
against the animator's simultaneous under/over combinations, each of capacity
24: mutually exclusive mouth/spiral/gaze resources do not consume one combined
budget. A complete validated immutable resource is constructed before it can
be activated. Corrupt packages do not reach rendering.

## Ownership, switching and display isolation

`Skin` owns dynamic palette, pose/layer tables and RLE bytes. The compiled
fallback uses the original static tables through the same borrowed pose view.
An animator owns an `Rc<Skin>`; each Scene retains its own `Rc`, while each
Layer copies its small image descriptor. There are no leaked allocations or
manufactured `'static` references. A Scene created before a switch remains
valid and renders with its original skin.

The renderer keys its composition cache by both scene composition and skin
resource identity. It retains one resource reference for the cached frame,
releasing it on replacement. Compiled-default resources share stable identity,
preserving the default animation schedule and cache behavior. A skin change
marks the widget urgent and the next render uses the new immutable resource.
The plane owns only an RGB565 canvas lease, so outstanding pixel transfers
cannot borrow a freed or mutated skin. Repeated switches have bounded live
resources: current control/animator, an optional old scene, and the renderer's
single cached resource, with normal reference-counted release.

Validation and filesystem work are outside interrupt critical sections. The
firmware's VFS uses its existing nonblocking busy marker; it does not disable
interrupts while reading or validating a package. The UI task prepares a whole
resource and calls `FoxControl::set_skin` as the single activation point.
Selection does not change the Home/navigation/settings rules or N2's terminal
Sleep state. N2's shutdown frame tag also checks the actual rendered Scene pose,
so the animator's old-face swap interval cannot count as displayed Sleep.

## Upload, persistence and recovery semantics

The wire protocol adds `skin-begin <id>`, `skin-commit <id>`,
`skin-select <id>`, `skin-rm <id>` and `skin-ls`. Skin staging is independent of
app staging. A half-upload, CRC failure or invalid staged package never changes
an installed skin or its selection.

A commit validates both files before replacing anything. An existing target is
renamed to `.previous/<id>`, then the complete staging directory is renamed to
the target. Rename failure attempts rollback; if rollback cannot complete, the
backup remains for recovery. Boot/selection recovery validates the target before
removing a backup. A missing/bad target with a valid backup restores the backup;
if neither validates, the backup is retained and fallback is reported. This is
a two-rename recovery protocol, not a claim of whole-directory atomicity across
arbitrary physical flash failures.

The selected ID is written and synced to `.selected.new`, closed, then renamed
over `.selected`. `skin-select` validates before this operation and requests UI
activation only after persistence succeeds. A write failure returns an error
and leaves the active/persisted choice unchanged. Boot reads the ID afresh;
missing selection means default, while invalid ID, missing package or corruption
falls back to default with a reason in the device log.

Removing the selected skin persists `default` first, then removes its files.
If persistence fails it does not remove the package. If file removal then fails,
the reply explicitly says selection may now be default and the UI still reloads
the durable selection; filesystem errors cannot leave RAM claiming a different
choice. The normal selected-delete path and restart both use the built-in fox.
Replacing a selected ID emits the same reload effect, avoiding stale in-memory
art after a successful upload.

## Validation and remaining acceptance

Offline gates (no device required):

```sh
cargo test -p mosaico-s31 --lib --offline fox::skin -- --nocapture
cargo test -p mosaico-s31 --lib --offline hub::skins -- --nocapture
cargo test -p mosaico-s31 --lib --offline hub::push -- --nocapture
cargo test -p mosaico-s31 --lib --offline shutdown_receipt -- --nocapture
python3 -B scripts/tests/test_skin_push.py
python3 -B scripts/tests/test_n1_device_session.py
cargo run -p mosaico-s31 --offline --example skin_check -- target/recolor/fox.bin
cargo fw --offline
cargo fmt --all -- --check
git diff --check
```

Tests cover default-pixel equivalence, changed recolor pixels, all pose/patch
rendering, cache invalidation, old-scene ownership and Weak-reference reclamation
through repeated switches; malformed headers, missing poses, bad dimensions,
offsets, RLE and palette; mutually exclusive versus simultaneous layer capacity;
real littlefs/VFS upload, reset during payload, staged replacement failure,
selection-write failure, same-ID replacement, cold selection restore, backup
recovery, missing/corrupt fallback and deletion. Python tests exercise the actual
upload API with fake serial responses and retain the bounded session tests.
`skin_check` validates a real packer output through the production Rust parser
and renderer, optionally writing RGB565 pixels as a second argument.

Evidence is recorded under `target/n3-acceptance-20261023`. Host verification and
a firmware build do not establish real USB transfer robustness, optical hot
switching, physical flash-power-loss recovery, reboot persistence on the target,
or device heap headroom. Those remain physical acceptance items, including
switching repeatedly while a frame is on the plane, selected deletion/reboot,
and rechecking N2 Sleep-before-cut after a skin change.

## 2026-10-23 independent acceptance receipt

The independent full `scripts/check.sh` run passed 317 host tests (218
`mosaico-s31`, 47 `mosaico-lua`, 10 `mosaico-fs`, 42 pure driver tests), both
previews, release firmware and 627-symbol ROM collision checks. The production
Python upload tests passed 3/3; the prior bounded-session suite passed 11/11.

Real packer outputs passed the production `skin_check` example and rendered
118,272 pixels each. The default package produced RGB565 CRC32 `d1e10f2f`; the
XOR acceptance palette produced `d5fbfd7e`. Their rendered images were inspected
in `target/n3-acceptance-20261023/skin-comparison.png`. The XOR variant deliberately
changes all palette colors, including black; it is a transport/render fixture.

| Artifact | Size / SHA-256 |
| --- | --- |
| `n3-default.bin` | 2,659,600 bytes; `a0e468ba8e85e6ecff4b3481d479cf3454c7ce75b64ebc678838481d99244075` |
| `n3-default.elf` | `842d6fbac03161af59ed315c38e23a99c901318bca45ebe711bd61db2b86df38` |
| Changed implementation source manifest | `0fe44b38ed01357178fd8c99a3cf3df5a9df0efe13b58dd7fe5c163ae4baadfd` |

`receipt.json` stores per-file SHA-256 entries against base `1b4cf33`, including
the earlier N2 actual-Sleep fix in that base. The source digest hashes sorted
`path + NUL + file_sha256 + LF` entries and excludes documentation. Saved artifacts
and logs remain under `target/n3-acceptance-20261023/`; this build was not flashed.
