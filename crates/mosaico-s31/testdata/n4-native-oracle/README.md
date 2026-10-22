# Native behavior oracle, 2026-10-23

Captured before N4 from the source files listed with SHA-256 in sources.json.
88 existing behavior tests passed unchanged assertions. Instrumented source
copies only rename the native classes and add forwarding wrappers that record
external calls/returns. Repository production sources were not modified.

native-contract.tsv is the raw Debug-value trace; native-contract.json converts
those values to typed JSON records (Some is unwrapped, None/unit are null).
Each case is a test name, engine is a unique instance id, and calls retain order.
The capture executable is a standalone rustc --test build, without Cargo/LVGL.

Use this corpus as a frozen N4 parity oracle. Failed Lua parity needs an
implementation fix or a separately justified documented intentional difference;
do not regenerate the expected outputs from the Lua implementation.

Runtime merge/shutdown/foreground task integration require separate checks;
this capture covers the actual nine behavior/tests suites, not every device path.

The tracked JSON is a whitespace-only compaction of the captured JSON.
`receipt.json` records both digests and the exact transform; `sources.json`
records the 12 native source file digests at the capture commit. The original
trace and capture workspace remain local acceptance artifacts under
`target/n4-native-oracle-20261023/`; the normal test needs only this tracked
fixture, never that ignored directory.
