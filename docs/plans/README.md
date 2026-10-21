# Development plans

> The grok-ball renderer has been replaced by the fox (`crates/mosaico-s31/src/fox`). Plans below keep their original wording; current structure is in [PROJECT_STRUCTURE.md](../PROJECT_STRUCTURE.md).

**Start here:** [Current state and remaining work](ROADMAP.md). Superseded plans, ledgers and the grok-ball execution order are archived in [`docs/legacy/plans/`](../legacy/plans/).

| Plan | Status | Scope | Execution record |
| --- | --- | --- | --- |
| [RFC 0001: Configurable Rust Component Library](component-library-development-plan.md) | Draft; implementation not started | Unified configuration; component selection; verified native providers and CI; scoped component composition | [Execution ledger](component-library-execution-ledger.md) |
| [RFC 0002: Mosaico UI Layer](mosaico-ui-layer-plan.md) | Implemented; host tests, RISC-V target build and gallery smoke run pass | Declarative `Node<Msg>`/`App`/`Ui` layer on LVGL 9.6; keyed lists; host gallery; S31 handoff boundary | [Execution ledger](mosaico-ui-layer-execution-ledger.md) |
| [RFC 0003: Grok-Ball First Screen (archived)](../legacy/plans/grok-ball-first-screen-plan.md) | G0–G2 complete on hardware; G3 optional | Rust/LVGL port of the MIT grok-ball engine; six AI states; 240×240 device canvas; full-screen transparent Obj touch layer; only the ball redraws; P95 30 ms / 20 fps | [Execution ledger](../legacy/plans/grok-ball-first-screen-execution-ledger.md) |
| [RFC 0004: Grok-Ball Sensor Reactions](grok-ball-sensor-reactions-plan.md) | Implemented and accepted on device (motor, camera/magnet, LDR, PIR); WS2812 mood light implemented (right slot); capacitive keys open; IR deferred | Interaction module in either slot, BMI270, BMM150 (both online), touch, keys and RMT haptics | [Execution ledger](../legacy/plans/grok-ball-sensor-reactions-execution-ledger.md) |
| [RFC 0005: Platform Runtime](platform-runtime-plan.md) | Complete; IMU on BMI270 FIFO at 50 Hz | `esp-rtos` + Embassy task skeleton, USB-OTG HS CDC console, async two-frame display/input, P95 gate, sensor task | [Execution ledger](../legacy/plans/platform-runtime-execution-ledger.md) |

## RFC 0002 compatibility report

The [LVGL widgets demo compatibility report](mosaico-ui-widget-compatibility.md)
records how the typed `lvgl-ui` layer maps to LVGL's official `widgets` demo:
which elements are typed, which use `Node::raw`, and which component or style
extensions are still missing.

## RFC 0001 at a glance

1. **One configuration:** native compilation, target bindings and Rust APIs agree; external configuration and custom fonts/C sources are supported through an explicit contract.
2. **Selectable components:** begin with Label + Button and Slider/Bar; verify disabled APIs, dependency closure, linked symbols and final artifact sizes.
3. **Reproducible builds:** separate vendored and verified external native providers; test feature combinations and real unpacked packages in a bounded CI matrix.
4. **Familiar composition:** add an optional typed component/building layer, Row/Column, events, tokens, variants and a real SDL gallery after lifecycle rules are proven.

W0 establishes acceptance, compatibility decisions and baseline evidence. W5 closes integrated acceptance and migration documentation. These surround the four product stages without adding another product direction.

## RFC 0002 at a glance

1. **One UI thread:** C callbacks enqueue messages; `App::update` changes state; `App::view` returns a `Node<Msg>` tree; the reconciler alone mutates LVGL.
2. **Typed composition:** Label, Button, Row, Column, Slider, Bar, Switch and Image; keyed dynamic lists; resolved style tokens; documented raw escape.
3. **Incremental delivery:** U0–U4 produce a host-tested `lvgl-ui` crate and target compile gate; Phase B adds S31 display/touch/RTOS; Phase C adds C3 and product services.
4. **Plan relationship:** RFC 0002 starts against the current `lvgl` 0.7.0 API and adopts RFC 0001 W4 behind the same public contract when that layer becomes available.

## RFC 0003 at a glance

1. **Ball first:** the first screen is a 320×320 grok-ball on a black field; the Slint-style Home/Apps screen becomes a later hub reference.
2. **Six AI states:** Idle, Listening, Thinking, Speaking, Happy and Error map to seeds `02`, `35`, `30`, `39`, `10` and `34`. RFC 0004 extends the reachable set to 14; the six AI states remain the voice/LLM mapping.
3. **Host-first engine:** a no_std `grok-ball` crate contains the data, math and polygon rasterizer; LVGL and `esp-hal` stay outside it.
4. **Custom widget:** `lvgl-ui` gains `CustomWidget` and `Ui::tick`; the ball canvas is updated only on the UI task.
5. **Pipeline dependency:** G0/G1 can start now; G2 device integration waits for the R0/R1 async pipeline and console gate.

## RFC 0004 at a glance

1. **Hardware truth:** Interaction module (type `0x16`) in either slot: left `EEPROM 0x50`, LDR/PIR/KEY_L/KEY_R `53/4/13/12`; right `EEPROM 0x51`, `46/5/11/10`. The camera (type `0x07`, EEPROM fixed `0x50`) works in the left slot only; BMI270, BMM150, CST9220 and the haptic motor stay on the main board.
2. **Two layers:** SensorTask publishes timestamped events; AffectEngine selects expressions with priority, cooldowns and return states.
3. **Reaction rules:** large shake → Panic → 5 s still → Confused; small bursts → Confused; 5–10 bursts → Angry 10 s; petting → Happy; light/PIR/keys/module hot-plug have explicit timelines.
4. **Fourteen reachable:** Idle, Sleep, Curious, Happy, Puzzled, Surprised, Panic, Satisfied, Confused, Angry, Listening, Thinking, Speaking, Error; the 32-expression catalog remains data-only for the rest.
5. **Haptics:** GPIO8 patterns accompany Panic, Angry, touch, module insertion and errors; the UI task never sleeps for a pattern.

Draft plans describe intended contracts, not available features. Writing or reviewing a plan does not authorize implementation or external publication. Only evidence-backed `complete` unlocks the next execution wave. Resume from the plan and its sidecar ledger; do not infer progress from a prior conversation.
