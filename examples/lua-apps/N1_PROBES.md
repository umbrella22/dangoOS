# Fixed N1 manual probes

The four historical apps preserve the launcher and Lua bytes used during the N1 manual
sessions. They depend only on firmware-provided modules: `delay`, `imu`,
`magnetometer`, `fuel_gauge` / `lib_fuel_gauge`, `haptic`, `led_strip`, and `button`.
No extra app files or third-party Lua dependencies are needed.

| App ID / directory | Fixed behavior | Expected result |
| --- | --- | --- |
| `n1_contract_probe` | Sensor snapshot/range checks, calibration roundtrip with restoration, fuel alias, two short haptic requests, LED pixel 0 low red / pixel 5 low green, active-key callbacks during a nominal 60 s window (maximum 120 s with args), then cleanup. | `[N1] SUMMARY` with zero FAIL; unavailable hardware or unobserved gestures may produce SKIP. Physical light, vibration and output timing require an observer. |
| `n1_lifecycle_probe` | Nominal 90 s window (maximum 120 s): right-board GPIO11/10 gestures, up to ten `error` haptic requests on single clicks; detach/reinsert checks old handles remain invalid, then reopens keys and both green LED endpoints; cleanup on normal completion. | `[N1L]` lifecycle sequence PASS after the instructed manual actions; missing manual actions produce SKIP, not physical acceptance. |
| `n1_error_probe` | Same Lua as lifecycle probe, but launcher fixes `mode="error"`: red/green endpoints for 3 s, then raises `N1_EXPECTED_FAILURE` without explicit cleanup. | Intentional runner error, VM drop and LED lease release. Run the normal lifecycle probe afterwards to check reopening; this error is expected, not a successful normal probe exit. |
| `n1_haptic_probe` | Wait 5 s, then four `angry` requests separated by 3 s (preset: two 160 ms bursts at 90% duty). | `[N1H] END` means requests completed only. Feel the motor independently; register receipts require a diagnostic firmware build. |

The additional `n1_haptic_rate_probe` is a finite request stimulus for a later
logic-analyzer check of the 150 ms pattern-start limit. It waits 5 s, issues
20 `thinking` requests with nominal 40 ms gaps, then waits 1 s for pending work.
The firmware preset is one 30 ms burst at 35% duty. Its only dependencies are
`delay` and `haptic`. Request logs are not actual pattern-start timestamps;
requests can coalesce, and script completion does not prove physical timing.
Measure actual motor waveform start spacing separately; this probe does not
read GPIO or registers and has no infinite loop.

Run a bounded console session from the repository root, using a **new** log path:

```sh
python3 scripts/n1-device-session.py --log target/n1-contract-new.log \
  --wait 60 --seconds 90 --command status --push examples/lua-apps/n1_contract_probe
```

`--command` repeats and executes in order before `--push`; uploading always runs
the app. `--port` overrides the existing runtime-port default
`/dev/cu.usbmodemmosaico_s31_r11`. `--wait` bounds connection retries and
`--seconds` bounds the final log window; both accept finite nonnegative seconds.
Each serial write has a 20 s timeout; protocol replies retain the existing push
tool's 20/30 s timeout. A
zero wait makes one connection attempt; a zero log window skips following.
The script imports the adjacent `push-app.py` without changing its protocol or
writing bytecode caches. `--help` works without pyserial; actual sessions need it.

Logs have UTC start/connection/finish markers and include errors. Existing log
paths are refused. Exit 0 means the transport session completed; it does **not**
parse Lua PASS/FAIL, certify expected-error cleanup, or establish physical
acceptance. Exit 1 indicates setup/protocol failure, 2 indicates serial/I/O or
CLI failure, and 130 indicates interruption. Missing protocol replies, explicit
`@err` responses and detected serial disconnects are failures. This wrapper has
no flashing functionality.
