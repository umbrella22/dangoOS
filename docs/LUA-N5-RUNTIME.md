# Lua image, LVGL and worker runtime

The foreground Lua VM communicates with native LVGL through a typed command
boundary. Native widgets run exclusively on the existing UI task; Lua callback
closures run exclusively on the Lua task. Workers execute in separate Lua states
and exchange bounded byte messages through their parent VM group.

```mermaid
flowchart LR
  F[Foreground Lua VM] -->|yield UiCommand| L[Lua task]
  L -->|one pending command| U[UI task / LuaLvglService]
  U -->|copied result and retired callback IDs| F
  P[Native pointer events] --> U
  U -->|bounded callback ID queue| E[process_events on Lua task]
  E --> F
  F --> G[Worker group]
  G --> W1[Independent Lua state]
  G --> W2[Independent Lua state]
  W1 <-->|binary queues / semaphore / lock| W2
```

## Image frames

`image.load_file` accepts baseline JPEG and PNG data. JPEG retains compressed
bytes until conversion or resize; PNG decodes to RGB888. PNG alpha is composited
over black with `(channel * alpha + 127) / 255` per channel. EXIF orientation is
not applied. TJpgDec determines supported baseline JPEG variants; progressive
JPEG is unsupported.

Frames expose `info`, `data`, `release`, GC and Lua `<close>` cleanup. Returned
strings and converted/resized frames remain independent after source release.
Native image buffers and decoder temporary allocations count against the same
per-VM heap cap as Lua allocations. Reallocation headers retain the allocating
VM identity. File input and JPEG encoded output are limited to 4 MiB; image
geometry is limited to 1920 × 1080 pixels and dimensions to 65535.

Conversion supports RGB565 LE/BE, RGB888, BGR888, GRAY8 and baseline JPEG/MJPEG.
JPEG encoding uses stb_image_write at quality 90. YUYV and UYVY constants remain
available for compatibility, but conversion to those formats raises an error.
Resize supports nearest and bilinear filters and outputs RGB565LE or GRAY8;
default output is RGB565LE, or GRAY8 for a GRAY8 source. `save_file` supports
`.jpg`/`.jpeg` and converts raw frames before writing. It rejects PNG output;
PNG encoding is unsupported. BIN uses the native LVGL decoder and is not an
`image.load_file` format.

Identity resize retains the decoded pixel conversion without interpolation.
Bilinear resize computes rational weights with bounded integer arithmetic;
nonbinary exact-half rounding cases retain the original double expression.
This preserves the previous byte-level RGB565LE and GRAY8 results across the
existing dimension and pixel-budget domain.

## Native LVGL bridge

`mosaico-lua/src/ui.rs` carries command JSON, an optional copied pixel blob and
the per-VM storage root. `tasks/lua.rs` suspends the VM until the UI task replies.
`lua_lvgl.rs` applies the command through LVGL 9.6. Handles contain monotonic
numeric IDs validated by a registered Lua userdata metatable; they never expose
native pointers. Display framebuffer mode and native LVGL mode are mutually
exclusive. Workers cannot acquire either display mode.

Publishing a pending command wakes the UI owner during its existing idle
interval. A fixed deadline and a bounded command count prevent a request stream
from extending that interval indefinitely. Input, animation and rendering run
in the ordinary UI loop between intervals. Stop and release also wake the owner;
the fast path observes their flags without consuming the normal teardown work.
A single native call remains synchronous and cannot be preempted mid-call.

The supported widgets are screen, object/container, label, button, canvas, image
and message box. The facade supports geometry, alignment, basic RGB/opacity/
border/padding style fields, flex, scrolling, text, screen loading, flags,
foreground ordering, canvas RGB565 upload and pixel operations, raw RGB565 image
upload, image pivot/rotation, message box footer buttons and event registration.
Unknown widget operations raise an explicit error. The full upstream widget,
font, chart and animation catalog is unsupported. `init` attaches to the
existing display runtime; upstream buffer/tick/task initialization options do
not reconfigure that runtime. Unknown style and widget-option keys raise errors. Message boxes require a
parent from the same service; screen creation requires no parent.

Canvas and raw image pixels are copied before a command completes. The aggregate
pixel budget is 2 MiB, including a conservative four bytes per pixel reservation
for decoded file images. There are at most 512 tracked objects, 32 callbacks per
object and 256 queued event IDs. Event overflow discards the oldest queued ID.
`process_events` dispatches callbacks with `pcall` so one callback error does not
stop subsequent callbacks. Deleting/cleaning native trees reports retired IDs to
release Lua closures. Native deletion callbacks only mark records dead; record
storage remains valid until the command completes. File-image cache entries are
dropped before their source strings or descriptors are released.

The service registers a read-only `D:` LVGL filesystem driver for the foreground
VM storage root. Relative paths reject empty, dot, parent and backslash segments.
Release restores the previous native screen, deletes tracked screen trees,
clears callback records and pixels, drops image caches and removes the drive.
The embedding must retain the previous screen throughout the service lifetime.
Leaving the Lua page, app teardown and shutdown request release through the UI
task. Host acceptance mirrors test data into libc and littlefs backends;
firmware uses the single VFS through libc syscall shims.

### Screen reconstruction and publication

Cleaning the active screen starts a reconstruction interval. The display refresh
timer is held so the last completed framebuffer remains visible. The same
display's running pointer read timers are paused, preventing a touch on an old
visible control from hitting an incomplete replacement tree. System raw touch,
edge gestures, AI shutdown and other LVGL timers continue through their existing
paths. LVGL normally resumes the display timer on each `REFR_REQUEST`, so a
display callback holds it again after the default handler. A paused timer by
itself does not indicate an external pause: LVGL also pauses it after an ordinary
refresh.

```mermaid
flowchart LR
  A[Clean active screen] --> B[Hold refresh and pointer read timers]
  B --> C[Apply widget requests; process system input]
  C --> D{Publication boundary}
  D -->|Event batch complete or script delay| E[Remove hold and request complete frame]
  D -->|Application exit| F[Release hold and restore native screen]
  style B fill:#fff3e0,color:#e65100
  style E fill:#c8e6c9,color:#1a5e20
  style F fill:#bbdefb,color:#0d47a1
```

`lvgl.process_events` sends `frame_end` after dispatching its callback batch,
including caught callback errors and calls with zero timeout. The foreground
runner also sends this boundary before a script delay when successful native
requests indicate a pending reconstruction. An individual failed widget request
leaves the interval open so Lua can recover before publication. `frame_end` is
an idempotent no-op after deinitialization or when no interval is pending.

At publication, layout is updated before pointer polling resumes, so the first
new touch uses the committed geometry even when its timer precedes refresh.
Input reset and `wait_release` discard contact belonging to the reconstruction
interval. Two guarded reads consume the port's one-read short-tap
latch and then its released state; a still-held contact must release before a new
press is accepted. Existing enabled flags and previously paused read timers are
preserved. Restoration checks that the saved input device, display and read timer
still match. This contract applies to the platform's timer-driven pointer input;
it does not intercept independent event-mode calls to `lv_indev_read`.

The refresh callback stores a stable context address until removal. Service
release restores the previous native screen and deletes the owned targets before
restoring pointer polling. It removes the callback before freeing that context.
The embedding must release or drop `LuaLvglService` before deleting its display.
Host probes cover real LVGL input and publication boundaries; physical display
continuity and rapid navigation require device acceptance.

### Native display profiling

The optional `native-ui-diagnostics` feature measures native Lua sessions with
frame IDs that follow each shadow-frame lease through submit, pickup, TE wait,
transfer and release. Measurement points use fixed storage; the UI task prints
bounded summaries outside the measurement critical sections. Per-window
completion counts become zero during idle, unlike the existing retained FPS
field. Frame records keep their originating session across a late completion;
mixed windows identify both session bounds.

Handler duration includes flush callbacks; flush duration measures copying into
the shadow frame. Transfer duration combines packing and SPI waits. Ordinary
UI work includes its native request and handler calls, while lease duration
overlaps work that the CPU can perform during transfer. These are wall-clock
measurements with containment and overlap, not independent CPU costs to sum.
The feature is disabled by default and does not change timer periods, display
buffers or TE policy.

## Independent workers

`mosaico-lua/src/workers.rs` schedules up to eight cooperative worker VMs per
foreground parent. Each state has a separate global environment and heap capped
at 1 MiB, inherits the parent storage root, receives JSON-decoded `args`, and
captures up to 8 KiB of print output. The group retains at most 32 job summaries;
errors are limited to 2048 Unicode scalar values. Jobs report textual status and
logs through `thread.start/run/list/get/stop`. Names support replacement and a
string exclusive group. Async timeout zero means no deadline. No operating
system threads or cross-state Lua object references are used.

Workers provide pure Lua libraries, JSON, delay and thread synchronization.
Hardware, image, display, LVGL, storage, IO, OS, debug, package searchers and
filesystem loading from worker code are unavailable. The trusted parent loads
the entry source before applying these restrictions. Nested worker creation is
unsupported. This capability boundary prevents native handles from escaping the
bounded shutdown path. Arbitrary script finalizers are skipped when the worker
state is destroyed, and unyieldable Lua execution has an uncatchable budget.
Ordinary foreground VMs retain their existing finalizer behavior.

Queues carry binary strings, including NUL bytes, with depth 1–32 and item size
1–4096. Queue capacity reservations total at most 256 KiB, with at most 32 queues.
Semaphores have maximum count 1–255; locks record the acquiring job ID. Each
registry has at most 32 entries. Sync names contain 1–31 ASCII letters, digits,
underscore, dot or hyphen and cannot refer to different resource types.
Timeout zero is nonblocking; finite waits retry cooperatively. Deletion rejects
queued messages, held locks or pending waiters with `busy`. A job stop completes a pending wait with `stopped`, allows one bounded
resumption, drops further requests and releases its locks before returning; parent interrupt, completion or
VM drop destroys the group and every child. The foreground driver must call
`poll_workers` during foreground sleeps and UI waits. Firmware does this at
one-millisecond intervals; `Vm::resume` also progresses the group each slice.
Deadlines use `Limits.clock` in firmware. Embeddings without that clock use a
deterministic logical millisecond per group tick and do not establish wall-clock
timeout behavior.

## Verification boundaries

The byte-identical original album and its Apache-2.0 license are stored under
`crates/mosaico-lua/vendor/album_app`. It enumerates JPG/JPEG/BIN and does not use
workers or enumerate PNG. Image and worker tests are independent of album tests.

Core host tests exercise exact PNG/alpha/RGB565 bytes, JPEG decode and encode,
corrupt input, frame independence, integer bounds and image limits. The unchanged
upstream thread harness covers API shape, nested JSON args, binary messaging,
named replacement, exclusive groups, contention, timeouts, stopping, 16 cleanup
cycles and concurrent producers/consumers. A hostile fixture checks capability
isolation, lock attribution and a nonterminating script finalizer during stop.
Native host probes exercise actual LVGL pointer events and repeated navigation.
Host results and firmware compilation do not establish physical display,
PSRAM pressure, real filesystem timing or on-device scheduler acceptance.

## Album photo delivery

`scripts/push-app.py photo-push image.jpg --name vacation/image.jpg --run`
publishes to `/data/appdata/album_app/photos/vacation/image.jpg` and optionally
opens the album. Names use the existing safe relative-path contract, at most
six components, and JPG/JPEG/PNG/BIN extensions; each upload is 1–1048576 bytes.
The original album enumerates JPG/JPEG/BIN only. A delivered PNG is available to
other image consumers but does not become an album tile.

The protocol is `photo-begin`, the existing CRC-protected `file` payload, then
`photo-commit <relative-name>`. Commit requires this session's successful
close-and-CRC receipt. A stale staging file alone cannot be published. A
same-filesystem rename preserves the old photo until commit. Disconnect,
explicit abort and N7 clear-data invalidate staging; a failed commit never
starts the album. Photo content is decoded by its consumer after delivery.

The CLI opens the POSIX console with DTR low, preserves input buffered during
open, then raises DTR. Photo payload writes continue until all bytes have been
accepted. A missing or mismatched ready/file receipt leaves the device's body
state unknown, so the CLI closes the connection without sending text cleanup.
An explicit file rejection permits `abort`; failed transfers never issue the
commit or optional album launch. The photo command closes its port on success
and failure, including after `--follow` ends.

## 2026-10-23 independent acceptance receipt

Base: `ab1be60772f26ec75bf8f5ffda794cb5c286a7e6`. Local artifacts and full source
manifest: `target/n5-acceptance-20261023/receipt.json`. The final host gate passed
347 tests (242 S31, 53 Lua, 10 FS, 42 pure drivers), both existing previews,
release firmware and the ROM collision check (713 C symbols). The Python tool
suite passed 25 tests. `git diff --check` passed.

Implementation manifest SHA-256:
`d3a76a7f1354feec658cec21a36f621d703f1d4f2a21973357c999ce9645b0e7`.

Three separate executable probes ran in external processes with a 30-second
timeout. `n5_album_preview` drove the unchanged factory script using real LVGL
pointer events: grid scrolling, JPEG/grayscale/BIN, corrupt image handling,
previous/next, deletion cancel/confirm, 48 grid/view cycles, external stop and
clean-then-set-text. All 11 screenshots were inspected after the button-label
fix. Late Lua heap low-water use was 186590 bytes, versus 186567 in the earlier
steady interval. Teardown restored the native screen, reported zero tracked
objects/pixels, and returned the counted Lua/native-image allocator to zero.

`n5_image_probe` and the independent Pillow oracle verified PNG RGB/alpha and
RGB565 bytes exactly. JPEG decoding/resizing had mean channel error 0.9271–3.2292
and zero pixels with channel error above 32. Encoded and saved JPEGs decoded in
Pillow with the correct geometry and orientation; comparison with Pillow's
quality-90, 4:2:0 encoding had mean error 0.0321/0.5938 and no pixels above 16.
The sharp color fixture's 3.16% of pixels above delta 48 against uncompressed
RGB565 also occurs with Pillow's own 4:2:0 encoder; raw-pixel equality is not an
appropriate JPEG encoding criterion.

`n5_worker_probe` verified progress during parent sleep, OOM containment,
deadline lock release, bounded history, two simultaneous groups with identical
resource names, and destruction of a blocked parent with a nonterminating
child/finalizer. All counted VM allocations returned to zero. The unchanged
upstream thread harness passed 14 cases independently of the album.

Firmware serialization explicitly used **16 MiB / DIO / 40 MHz**, and esptool
validated chip 32, checksum and embedded SHA. BIN: **2987056 bytes**, leaving
**158672 bytes** in a 3 MiB slot. SHA-256:

```text
BIN ac2b42b743c2eb76e01d8c93ba982c57e78ca93a3325b7439ff3533105100885
ELF 3f616cc070a6dfdae96ada1821fa6c3835bd53c15b64b426da9ac105e6bf55a3
```

Reproduce the native probes after building their examples with Cargo. Image
fixtures are tracked under `crates/mosaico-lua/testdata/n5-images`; the original
script is `crates/mosaico-lua/vendor/album_app/scripts/album_app.lua`. Album and
worker output directories must be new. Run
`python3 scripts/n5-check-images.py <image-probe-output>` with Pillow installed.
The host album harness mirrors photos into libc and LittleFS for the native
`D:` driver; on-device filesystem routing and timing remain **not-run**.
