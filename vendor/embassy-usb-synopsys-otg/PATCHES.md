# Local Synopsys OTG patches

This package vendors the actual `embassy-usb-synopsys-otg` 0.4.0 source previously compiled by the ESP32-S31 firmware. `LOCAL_PROVENANCE.json` records every cached file hash and the upstream metadata commit, `664d4ead36bb24a63955ca649bcec66c6e70bf6d`. The shared Cargo cache is unchanged. The root `[patch.crates-io]` makes this dependency reproducible from the repository.

The cached source already contained non-EP0 OUT priming when an endpoint is enabled, plus IN SNAK on enable and FIFO flush behavior. Those changes are preserved. All original cached files other than `src/lib.rs` remain byte-identical; `src/reset_in.rs` adds the shutdown state machine and its host tests. MIT and Apache license texts come from the upstream metadata commit.

## IN transfer shutdown before USB reset reconfiguration

A captured reconnect reached `configured(true)` while the application TX task was at `TxWait`, yet bulk IN EPENA remained set with the old transfer state. A subsequent write remained pending with enough FIFO space. This supports an old IN activation surviving reset reconfiguration. Hardware acceptance of this candidate change remains pending.

On USBRST, the bus now completes this sequence for allocated non-control IN endpoints before the existing FIFO flush and endpoint configuration:

1. Set SNAK and wait for persistent `DIEPCTL.NAKSTS`.
2. Recheck EPENA in a short critical section. A naturally completed transfer skips EPDIS and returns to Idle.
3. Clear only the stale EPDISD acknowledgement, request SNAK/EPDIS, then clear USBAEP in the existing disable order.
4. Wait for EPENA to clear and for raw `DIEPINT.EPDISD` or an IRQ-latched EPDISD acknowledgement.
5. Run the original `init_fifo()` and `configure_endpoints()` only after all affected IN endpoints complete.

The IRQ handler records EPDISD before its existing W1C acknowledgement. The poll also checks raw status because the existing interrupt mask does not enable EPDISD. A consumed or masked interrupt therefore cannot be the sole completion signal.

Each hardware wait returns `Poll::Pending` and self-wakes. No new spin loop or long critical-section wait is added. There is no timeout or fallback: a missing hardware acknowledgement can leave USB reset/enumeration pending and consume polling time. Other executor tasks can still be scheduled. Existing FIFO-flush waits remain unchanged.

EP0, OUT handling, normal `write()`, SET_CONFIGURATION, packet contents, VBUS/DTR policy, retries and OTA behavior are unchanged. Hardware validation checks immediate command recovery after reconnect and an inactive IN endpoint at configuration. An old DIEPTSIZ value alone does not establish failure once EPENA is clear and a subsequent write overwrites it normally. A closed host port can still leave an ordinary IN transfer pending.

## References and verification

- [Linux v6.12 DWC2 `dwc2_hsotg_ep_stop_xfr`](https://github.com/torvalds/linux/blob/v6.12/drivers/usb/dwc2/gadget.c#L3914): NAK effective, EPDIS, Endpoint Disabled, then FIFO flush. Its timeout and fallback policy are not imported.
- The original driver’s isochronous IN workaround also waits for NAK effect and EPDISD before continuing.
- The pinned ESP32-S31 PAC describes NAKSTS as the core transmitting NAK, requires EPENA to be set before requesting EPDIS, and requires Endpoint Disabled acknowledgement. EPENA clears before Endpoint Disabled or Transfer Complete.

Two pure state-machine host tests cover the required ordering, incomplete acknowledgement, natural completion, and repeated polling. Default and v3 diagnostic firmware builds, image validation and ROM checks pass. Device reconnect acceptance remains required. Frozen before/after sources, cache-to-vendor diff, references and both image sets are archived under `target/usb-in-reset-fix-20261024`.
