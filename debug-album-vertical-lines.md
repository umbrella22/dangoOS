# Album vertical lines

Status: [DEFERRED] Unresolved; operator chose to restore 80 MHz and continue acceptance.
Session: `album-vertical-lines`
Hardware: ESP-Mosaico CoreBoard V1.2, right interaction board.
Initial firmware: `8c9b37f458692767216d02c0409c74cf269cd997ba6cb4b5565db9ab50aa0b92`.
Current installation and serial ownership: `target/n5-render-live-20261025/resume-state.json`.

## Report

During the valid oversized JPEG acceptance the user reported persistent vertical
lines inside Album, beside the dialog and through the Load failed region. The
system screen is normal after leaving Album. General navigation appeared normal.
Capture 43 records deletion of the exact oversized fixture. Capture 44 verifies
cleanup separately; this is not a visual fix.

## Falsifiable hypotheses

1. Scrollbars on otherwise static nested objects: compare scrollbar mode,
   scrollable flags, scroll extent, and line coordinates in error/grid/dialog frames.
2. Content overflow or clipping: compare bounds, padding, text extent, and clip
   areas; verify the line follows a specific object's geometry.
3. Partial redraw residue: compare the native live frame with a forced full redraw
   of the same object tree and repeat page changes.

## Evidence plan

Use the existing real LVGL host harness with the frozen factory Album, valid
fixtures, and a temporary malformed image. Instrument host object geometry and
render state before changes. Keep production and vendor logic unchanged until
the evidence identifies a cause. Retain raw frames, request traces and local HTTP
debug events under the session. Compare any isolated candidate against the same
inputs and controls; physical confirmation remains required.

| Hypothesis | Likelihood / effort | Observation |
| --- | --- | --- |
| A: extra scrollbars | medium / low | Native object scroll extent and scrollbar rectangle match the line |
| B: clipping overflow | medium / low | Child bounds exceed static content area; forced redraw keeps the same clipped edge |
| C: stale partial redraw | medium / medium | Incremental frame differs from full redraw at the line, with the same object tree |

Local HTTP collector: `http://127.0.0.1:7777/event`, managed exec session `12035`.
Environment: `target/n5-vertical-lines-20261025/.dbg/album-vertical-lines.env`.
New host instrumentation will report object geometry and pixel comparisons using
this collector. Existing serial acceptance records remain separately retained.

Capture 44 completed: oversized and malformed fixtures are absent, valid 02–20
and factory files match their byte counts/CRC, temporary verifier removed.
This only closes storage cleanup. The user's visual report remains open.

## Current state

Seven pre-fix native host frames were instrumented. HTTP log lines 2–8 record
grid, error fullscreen, error Delete dialog, Cancel, valid image, valid Delete
dialog and grid return. The normal grid scrollbar occupies x=462..465,
y=48..440. Error/fullscreen/dialog objects have no valid scrollbar rectangles;
status content height is 16 pixels, matching the font line height. The 150-pixel
msgbox contains its header/content/footer and has no observed clipping.
All seven incremental frames equal their forced full-redraw frames exactly.

| Hypothesis | Result | Limit |
| --- | --- | --- |
| A: extra scrollbars | Not reproduced on host | Physical line coordinates unavailable |
| B: clipping overflow | Not reproduced on host | Device rendering may differ |
| C: stale partial redraw | Not reproduced on host | Host frame comparison excludes the panel upload path |

Artifacts: `target/n5-album-lines-host-20261025/pre-fix/`.
The retained build outputs point to an older local LVGL source directory. Current
repository HEAD must not be used as proof of the exact historical dependency
source. Configuration and retained-artifact comparison are being recorded with
that limitation.

Capture 45 opens original Album with the nineteen valid fixtures for a photograph
of the actual line. Serial owner session `63771`; the app retains its original
180-second lifetime, and may be reopened manually. No photo deletion requested.
The root cause remains open. No visual fix applied or flashed. Photo transfer
scenarios remain paused to address this reported defect.

## Exact sequence supplied after the initial host run

The first Delete dialog is clean. Cancelling it leaves a vertical line at the
dialog's former left edge. Switching to the next image and opening Delete again
reveals shifted remnants against the new white dialog. This is a sequence-dependent
symptom, so the earlier forced full refresh after each captured host stage is an
insufficient reproduction. Repeat the whole pointer sequence without any forced
redraw between stages; run the full-redraw comparison in a separate fresh run.
Record both the LVGL shadow frame and a simulated panel updated only through the
device's damage union, four-column expansion and packed upload windows.

The user's exact sequence has been submitted to the HTTP collector as
`runId=pre-fix-sequence`. A photograph is no longer the sole next step.

## Continuous sequence and next observation boundary

The continuous sequence completed in four independent host runs. Root verified
225 retained artifacts, all 30 shadow/panel comparisons, and the corrected V2
terminal refresh control. Cancel flushes cover the old left edge; the independent
terminal full refresh changes zero pixels. This does not reproduce the physical
SPI/DMA/cache/panel boundary. No rendering fix has been selected.

Capture45 has completed and its raw/log/event fingerprints were verified. The
serial port is free. A feature-gated diagnostic candidate is being prepared to
sample stable shadow-frame and per-column CRCs after popup open/close. Sampling
waits for frame_end, a later handler, 450ms quiet and no outstanding frame lease
or damage. Output is bounded to eight samples per boot, with explicit skipped or
cancelled records. Serial events will be relayed to the retained HTTP collector;
no new USB command or device storage fixture is needed.

## Diagnostic candidate prepared; physical capture pending

Candidate: `target/n5-popup-shadow-diag-20261025/build-v2/diagnostic.bin`,
3,085,936 bytes, SHA256
`ddd0cfe65cab2fa3cb99be374c82126936913bd1354abb977a1dcea8add42b84`.
Root source/artifact review: same directory `root-review.json`. All545 inputs
are unchanged during checks; four existing-file cfg-gated observation changes
and one diagnostic module are the only source delta from the draw48 seal.
Project424 checks, six hook/state tests,24 independent frame CRCs and11520
column CRCs pass. Both firmware variants build and validate. No physical fix or
new flash has run yet. The first packaging helper's wrong ELF basename is
preserved as a failed receipt; V2 resumes from its successful same-source full
check and completes image packaging.

The root prepared one-write main_app flash script (`flash_reviewed_v6.py`) and
`popup-shadow-prepared.json`. The operator has been asked to enter ROM download
mode. Stop/abort remains available; no current serial owner. After full readback
and preserved boot verification, automatic reboot uses46-popup-shadow-startup.
Next acquisition uses original Album, existing nineteen images and popup actions
spaced at least5seconds. Each capture must include all480 column CRCs and matching
end before comparison; truncated logs are incomplete evidence. Serial-to-HTTP
relay and complete-stream parser were separately checked.

## Diagnostic installed; live sequence capture47

The operator entered ROM mode. `flash-06/root-verified.json` independently
confirms all3,085,936 application bytes equal the reviewed image, boot0..0x20000
equals the pinned backup before and after, and the prior header equals draw48.
One automatic watchdog reset succeeded. Startup46 contains the new popup CRC
marker, right interact descriptor error0, default brain/skin and original four
downloaded apps; status reports system12/750 and data436/512.

Original Album capture47 reports19 images. Serial owner is58733; stop only via
`47-popup-shadow-sequence.stop` before opening any other serial handle. Read-only
HTTP relay is95871 and ends after the collector receipt. Operator is asked to
open02, Delete, Cancel, next03, Delete, waiting at least5seconds each and leaving
the second dialog open. No photo deletion or new device application is requested.
Physical root cause/fix remains OPEN pending complete CRC streams and observation.

## Controlled device observation localizes the boundary

Capture48's controlled q9 open and q10 close each contains begin,all480 column
CRCs and matching end; q11 reopened dialog also completes. q9/q11 full CRC
0efb7df8 plus every column matches the correct host02-first-delete frame; q10
full CRCb4c8966d plus every column matches the correct fullscreen02 frame.
The operator explicitly reports a visible line while q9 is held and persistence
after Cancel. `48-popup-shadow-root-review.json` binds raw evidence, installed
image and operator statements. CRC equality is not a physical panel readback.

This rules out a stale line already encoded in the sampled shadow for these
paired observations. Investigation now targets post-shadow packing/staging,
HAL/SPI/window transfer and panel timing. The prior q4 difference atx403..468
is not classified as a button state; unsynchronized fast actions are excluded
from this controlled conclusion. The unexpected page exit corresponds to
original Album's normal180753ms completion and a fresh start3.3seconds later.

The official CO5300 QSPI configuration macro uses40MHz, while the installed
firmware uses80MHz for register and pixel transfers. This is a documented
default difference, not proof of a hardware maximum or a clock root cause.
A new single-variable40MHz candidate is being prepared, retaining32-row chunks,
window geometry, packing, DMA, UI and CRC diagnostics. Fix acceptance remains
OPEN until physical comparison. No current serial owner; all eight diagnostic
samples for this boot have been consumed.

##40MHz candidate ready for physical comparison

Source delta is exactly SPI_CLOCK_HZ80_000_000→40_000_000 after comments and
whitespace normalization. Both MCU variants build,validate as ESP32-S31
16MiB/DIO/40MHz flash images and pass714C-symbol ROM collision checks. This
LCD transport clock change does not alter flash configuration.545build inputs
remain stable; onlydisplay.rs differs from installed80MHz diagnostic.
Unchanged host matrices are retained,not rerun.

Candidate `target/n5-popup-transport-40mhz-20261025/build/diagnostic.bin`:
3,085,744B,SHA256c94c37b44c7801e21cba455836f3e3b050f00374233b8b7376a763c38805ae7f.
Root review and30preparation artifact fingerprints verified.
`popup40-prepared.json` and `flash_reviewed_v7.py` are ready forflash07;
`root_verify_flash07.py` requires the49startup log to reportQSPI@40MHz and
the unchanged CRC marker. Operator has been asked to enter ROM mode.
Installed image remainsddd0cfe6 until that write is verified.40MHz physical
result remains not-run; old image retained for comparison or rollback.

## 40 MHz comparison completed; investigation deferred

`flash-07/root-verified.json` confirms the full 3,085,744-byte application readback,
unchanged boot region, automatic restart and startup at QSPI 40 MHz. Capture 50
is stopped and sealed, with eight complete frame/480-column CRC samples, no
parser errors, and a completed HTTP relay. The first three samples match the
correct photo 03 dialog/fullscreen/dialog frames. Other rapid interactions are
not treated as a controlled photo 02 sequence or as physical panel readback.

The operator reported that the first dialog looked correct at 40 MHz, and that
navigation was slightly slower but acceptable. They then explicitly chose:
“恢复 80 MHz，暂缓残影排查，继续其他验收”. The broader cancel/switch sequence
was not individually confirmed. The comparison shows an observed improvement;
it does not establish a specific transport cause or an accepted final fix.

At 480×480 RGB565, raw four-line pixel transfer takes 23.04 ms at 40 MHz or
11.52 ms at 80 MHz. These exclude rendering, commands and waits. Thus 40 MHz
cannot transfer 60 complete frames per second, while partial updates may differ;
neither calculation establishes actual application FPS. Restore the ordinary
80 MHz firmware to preserve the bandwidth target, with diagnostic sampling
excluded by its existing feature gate. Retain the source and evidence for a
possible future investigation. No further residue experiments are scheduled.

Evidence: `50-popup40-root-review.json`, `50-popup40-analysis.json`, capture 50
and its relay receipts. Restoration preparation is in
`target/n5-restore80-20261025/`; device restoration is not complete until the
next flash, full readback, boot preservation and runtime verification pass.

Restoration is now verified by `flash-08/root-verified.json`: ordinary 80 MHz
image SHA256 `0551aa2264be2956b7854be4ea433d615aaf184b9fc237cd3c179c79c29b978c`,
3,063,568 bytes, exact application readback and unchanged boot region. Startup 51
reports QSPI@80MHz, default brain/skin and right-slot interact with no descriptor
errors. Popup CRC/native-heap diagnostic sampling is absent. The visual issue
remains deferred and unresolved; subsequent acceptance concerns photo delivery.
