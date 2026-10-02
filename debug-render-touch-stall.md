# Debug Session: render-touch-stall

- **Status**: [OPEN]
- **Issue**: Touch input still lags on hardware after v20. Reported render averages are 348–471 ms; the maximum touch polling gap is 526.525 ms.
- **Debug Server**: http://127.0.0.1:7777/event
- **Log File**: .dbg/trae-debug-log-render-touch-stall.ndjson

## Reproduction Steps

1. Run the 800×480 firmware through startup and regular one-second telemetry updates.
2. Select manual fan speeds and AUTO while recording heartbeat and touch events.
3. Compare render phases, dirty pixels, allocation/raster work, and time spent in LCD refill interrupts.

## Hypotheses & Verification

| ID | Hypothesis | Likelihood | Effort | Evidence |
| --- | --- | --- | --- | --- |
| A | Complete telemetry updates dirty substantially more of the screen than the history-only probe. | High | Low | Confirmed: MCU steady windows average 90,633 dirty pixels. No full redraws after startup. |
| B | Path rasterization, temporary allocations, or render traversal dominate elapsed time despite small dirty areas. | High | Medium | Confirmed: MCU paths average 129,186 us for 29 segments and 1,712 mask pixels per frame; dirty computation adds 64,396 us. Host full ticks average 1,658 allocations. PSRAM placement's share still requires a controlled target comparison. |
| C | LCD refill interrupts and PSRAM contention significantly extend render wall time. | Medium | Medium | ISR copying averages 97,561 us within 334,525 us rendering (29.2%). The remaining 236,964 us cannot be explained by copying alone. Contention outside the ISR is not measured separately. |
| D | The installed firmware or timing boundaries differ from assumptions. | Medium | Low | Rejected for this capture: startup confirms v21-probe, CPU=320MHz and PSRAM-only heap. |

## Log Evidence

User serial log, 2026-09-20 18:06:21–18:06:42:

- Render averages: 348,255–471,338 us per heartbeat window.
- Frame publication averages: 14,739–25,574 us.
- Maximum touch poll gap/read duration: 526,525/3,512 us (lifetime maxima).
- EOF mismatch, underrun, DMA errors and publication aborts remain zero.
- Input and rendering currently share one synchronous loop.

## Verification Conclusion

The observed render duration alone explains long gaps between input polls. The previous history-only host measurements did not establish full UI latency on the MCU. No serial board is attached to this workspace; hardware instrumentation will be packaged for replay if local evidence cannot settle the target-specific cost.

## Instrumentation and Host Evidence

The first existing-code change added counters and logging only. The diagnostic firmware adds no rendering, memory-placement or input-policy fix. MCU logs use its existing UART transport; the MCU has no HTTP stack. Host probe events and the supplied MCU summary are posted to the local collector. Additional hardware UART records will be imported into the same session.

Host probe: `target/render-touch-stall/src/main.rs`; run IDs `pre-fix` and `pre-fix-phases`. The latter includes nested phase instrumentation. Debug-build host microseconds are not MCU measurements.

| Host scenario | Average dirty pixels | Average allocations | Average host render time |
| --- | ---: | ---: | ---: |
| Complete manual-mode telemetry tick | 91,639 | 1,658 | 3,606 us (profiled) |
| Speed selection | 113,131 | 1,116 | 4,275 us (profiled) |
| History only | 2,139 | 1,456 | 1,458 us (profiled) |
| Uptime only | 1,628 | 649 | 866 us (profiled) |

Average full-tick phases: setup 38 us, dirty computation 812 us, background 212 us, scene 2,541 us. Text 1,128 us and paths 531 us are nested inside scene; mask 222 us is nested inside paths. They must not be summed as independent phases.

The hardware currently puts its global allocator entirely in PSRAM. The v21 target measurements below confirm that small-pixel-count, allocation-heavy work is expensive; internal-RAM placement is the next controlled fix to evaluate.

The v21-probe `[perf]` line records frame count, dirty pixels, full redraw count, ISR copy time during rendering, per-window touch timing, and rendering phases. The regular lifetime touch counters remain available. UART reporting is outside measured rendering phases and still contributes to main-loop wall time.

## Device Handoff

- `scripts/check.sh` passed all 29 tests, both desktop layouts, the MCU release build and runtime dependency isolation.
- Package: `releases/fan-control-v21-probe-20260920.zip`.
- Application: `releases/fan-control-v21-probe-20260920/fan-control-v21-probe-app.bin` at `0x10000`.
- Package checksums, source hashes and ZIP integrity verified; hardware verification remains pending.
- Initial host and supplied-board evidence is copied to `target/render-touch-stall/pre-host.ndjson`. The collector log is cleared for the target reproduction.
- Next evidence: startup version/CPU line plus heartbeat and `[perf]` pairs after 40 seconds idle and during 25/100/AUTO selections.

## v21 Target Evidence (received 2026-09-20)

Raw capture: `target/render-touch-stall/v21-idle.txt` (preserved verbatim).
Imported run: `board-v21-pre-fix` in `.dbg/trae-debug-log-render-touch-stall.ndjson`.
The collector was restarted without `--clean` after its previous idle timeout.

Lines 33–34 confirm v21-probe and CPU=320MHz. Lines 109–129 contain eleven
steady-state windows with 2 frames each and 58 path calls per window:

| Metric | Mean | Range |
| --- | ---: | ---: |
| Render | 334,525 us | 333,640–336,841 us |
| Dirty computation | 64,396 us | 63,701–65,045 us |
| Scene | 260,741 us | 259,698–262,880 us |
| Paths (inside scene) | 129,186 us | 128,514–131,022 us |
| Mask generation (inside paths) | 58,469 us | 56,289–59,205 us |
| ISR copying (inside render) | 97,561 us | 97,012–98,525 us |
| Window maximum touch polling gap | 348,083 us | 346,166–350,400 us |
| Window maximum I²C read | 1,713 us | 1,522–1,967 us |
| Dirty pixels per frame | 90,633 | 90,106–91,326 |
| Mask pixels per frame | 1,712 | 1,709–1,715 |

All steady windows have `full=0`. EOF mismatch, underrun, DMA errors and
publication aborts remain zero throughout the capture. Early history growth
(lines 49–57) increases path calls from 5 to 21 per window and path time from
14,314 to 45,975 us per frame, despite only 1,015–1,235 mask pixels. Speed-change
windows (lines 68–99) increase render averages to 319–407 ms.

The evidence confirms a synchronous-render input stall and rules out continuing
full-screen invalidation or slow touch bus reads as its main source. The old
539,633 us touch maximum includes startup; compare the resettable `touch_us`
window field instead. Nested phases and ISR time must not be added twice.

The v21 ELF reserves 369,184 bytes between `.bss` and the main stack top. A
256 KiB internal UI heap leaves about 104 KiB for the main stack and interrupts.
Framebuffers must use explicit `MemoryCapability::External` allocations before
introducing an internal-first global heap. Internal memory usage and external
fallback will be reported in the next build; board timing remains to be verified.

## v22 Fix and Host Comparison

Changes after the target evidence gate:

1. Register a 256 KiB internal heap before PSRAM. Normal allocations prefer SRAM
   with external fallback. Framebuffer allocations explicitly require External,
   are 128-byte aligned and are initialized before any slice or ISR publication.
2. Retain one mask and `zeno::Scratch` per software renderer. Clear the active
   mask for each draw, recompute coverage, and preserve the original coordinates
   and union-of-dirty-regions blending. This avoids repeated mask allocation and
   Zeno's default 1,024-cell stack workspace initialization.
3. Preserve all v21 counters, update the startup identifier to v22, and append
   internal/external heap used/free snapshots to `[perf]`.

Pre-change source snapshots are in `target/render-touch-stall/v21-source`.
Host runs `pre-fix-memory` and `post-fix-v22` use the same replay and allocation
instrumentation. These measurements evaluate workspace reuse, not MCU SRAM speed:

| Scenario | Allocations/frame before → after | Dirty pixels before → after | Host mask time before → after |
| --- | ---: | ---: | ---: |
| Full telemetry (60 frames) | 1,657.9 → 1,629.2 | 91,639.3 → 91,639.3 | 217.3 → 126.1 us |
| Speed changes (10 frames) | 1,116.3 → 1,101.4 | 113,131 → 113,131 | 273.8 → 248.6 us |
| History only (10 frames) | 1,456 → 1,427 | 2,139.2 → 2,139.2 | 233.6 → 134.1 us |
| Uptime only (10 frames) | 649 → 649 | 1,628 → 1,628 | no paths |

Host live heap excluding framebuffers increases by 10,200 bytes for retained
working capacity (about 630 kB → 640 kB in this 64-bit host process). This host
figure includes runtime allocations and is not a 32-bit MCU heap requirement.
The firmware's heap counters are needed to observe actual internal usage and
external fallback. No target post-fix measurement is available yet.

The reusable-mask regression compares fresh `Mask::new` with reused scratch
across 30 combinations of size, style and path; all masks are exactly equal.
Existing fill/stroke dirty-region tests also pass. `scripts/check.sh` passes all
30 tests, both desktop layouts, MCU release compilation and runtime dependency
isolation. The physical board is not attached to this workspace.

## v22 Device Handoff

- Final ELF: internal heap is a static 262,144-byte object at `0x2f012b00`;
  `.stack` spans `0x2f052d90..0x2f06cfb0` (107,040 bytes). This is a linked
  memory budget, not measured stack high-water usage. Layout is preserved in
  `target/render-touch-stall/v22-elf-layout.json`.
- Checks: `target/render-touch-stall/v22-check.log`, all 30 tests and builds pass.
- Package: `releases/fan-control-v22-20260920.zip`.
- App: `releases/fan-control-v22-20260920/fan-control-v22-app.bin`, 2,555,536 bytes,
  flash offset `0x10000`.
- Current source hashes, exact built ELF, package SHA256 values, ZIP integrity
  and all archived file contents were verified.
- Both independent code-review validators confirmed the allocation and workspace
  changes and found no release-blocking regressions. Both explicitly distinguish
  the measured synchronous-render stall from the still-unmeasured SRAM speedup.
- Next target comparison: wait 40 seconds after boot, capture five idle windows,
  then 25/100/AUTO changes and five more heartbeat/perf pairs, including the
  startup configuration and heap fields. Target improvement is not yet claimed.
- Session remains [OPEN]; instrumentation and evidence are retained until
  hardware feedback confirms resolution or the session is explicitly aborted.
