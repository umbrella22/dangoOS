This package is copied from esp-rs/esp-wifi-sys revision
`5afc3a38cc1abcd463e3b5140f0ac742de44053f`. `UPSTREAM.json` records every
imported file hash. The input binary archives and generated bindings are unchanged.

Integration changes are confined to `build.rs`: the copied `libprintf.a`
gets private names for `printf`, `sprintf`, `snprintf`, `vsnprintf` and `vprintf`,
including references inside that archive. Vendor logging entry points remain
available. Lua's standard C formatting continues to resolve to full newlib.
The copies of all archives also bind their `gettimeofday`, `sleep` and `usleep`
calls directly to esp-radio's `__esp_radio_*` adapters. The upstream `PROVIDE`
aliases otherwise lose to newlib and pull in OS syscalls that do not belong to
the controller's scheduler/boot-clock contract. This does not implement a Unix
wall clock for applications.

The build copy of `libbredr_app.a` weakens `r_co_assert_info`, permitting the
firmware's strong implementation to retain the controller assertion in RTC
panic storage before reset. The ABI is the pinned archive's four arguments
(file pointer, line, two integer values). This preserves fatal failure and
avoids losing the cause in the controller's UART-only report and null store.
The original archives on disk are unchanged.

This needs the matching RISC-V `objcopy` from the configured C toolchain.
No Cargo checkout/cache or source archive is modified.

Upgrade this package together with esp-radio/PHY. Re-evaluate and remove the
formatter patch when upstream offers a namespace or newlib integration. S31
Wi-Fi modem sleep is disabled by the firmware while upstream retention callbacks
are incomplete. WPA3-only association is not exposed by the current Rust backend.
