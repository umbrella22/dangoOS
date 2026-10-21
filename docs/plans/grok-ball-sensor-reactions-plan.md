# RFC 0004: Grok-Ball Sensor Reactions — Interaction Module, IMU, Touch and Haptics

> **当前状态（2026-10-21，覆盖下方原始草案的 Status 行）**
>
> - 渲染对象已从 grok-ball 换成狐狸团子（`fox/`），grok-ball 代码已删除；本文的 BallExpression 即现在的 `behavior::Expression`。
> - 反应层已实现并有 host 测试：`behavior/affect.rs`（摇晃、轻拍、光线、PIR、KEY_L/KEY_R、磁铁、电量、模块插拔），测试在 `behavior/tests/`。
> - 模块驱动已改为左右槽自适应：`drivers/subboard/`。交互板插左（`0x50`）或插右（`0x51`）都能用；同时只读一块；相机固定按左槽报告。`ModuleInserted/ModuleRemoved` 现在带 `ModuleKind`，反应跟随模块类型而不是槽位。下文 §2.2、§7 里"交互板只在右槽、相机只在左槽"的假设已经放宽，规则见 [硬件文档 §7.2.1](../MOSAICO_V12_HARDWARE.md)。
> - 仍未完成：交互板 LDR/PIR/按键在真机上的逐项验收；WS2812 心情灯；SI12T 触摸键模式；IR 发射（本 RFC 原本就不使用）。

> Original status (2026-10-20): Draft implementation plan; no implementation work package is complete.
> Plan ID: `mosaico-grok-ball-reactions-v1`; revision: `1`.
> Baseline date: `2026-10-20` UTC.
> Related plans: [RFC 0003 Grok-Ball First Screen](grok-ball-first-screen-plan.md), [RFC 0002 Mosaico UI Layer](mosaico-ui-layer-plan.md).
> Hardware baseline: official `esp-mosaico-claw` board configuration and `esp-mosaico-bsp` Interaction module; Interaction module in the right slot, camera in the left slot.
> Implementation authorization: granted later by the user; see the current status above.

Navigation: [Purpose](#1-purpose-and-runtime-rule) · [Hardware](#2-hardware-truth) · [Reaction layers](#3-reaction-layers) · [Event rules](#4-event-rules) · [Expression catalog](#5-expression-catalog) · [Haptics](#6-haptic-feedback) · [Module lifecycle](#7-module-lifecycle) · [Testing](#8-testing-and-acceptance) · [Milestones](#9-milestones-and-estimates) · [Risks](#10-risks-and-open-decisions) · [Handoff](#11-handoff)

## 1. Purpose and runtime rule

RFC 0004 defines how physical sensors change the grok-ball expression. It covers the right-slot Interaction module, the BMI270 IMU, the two BMM150 magnetometers, the CST9220 screen touch, the board buttons and the vibration motor. The ball renderer remains RFC 0003's responsibility.

The reaction layer translates sensor streams into an `AffectState`:

```text
IMU / LDR / PIR / keys / touch / buttons / BMM150 / module presence
        -> SensorTask
        -> SensorEvent { timestamp_ms, source, kind, strength }
        -> AffectEngine
        -> BallExpression + HapticPattern
```

The primary runtime rule:

**SensorTask samples hardware and publishes timestamped events. AffectEngine is pure Rust and makes every expression decision. BallWidget renders the selected expression. HapticPattern drives GPIO8. None of these steps calls LVGL outside the UI task.**

SensorTask never calls LVGL. AffectEngine never reads hardware. The ball renders one expression at a time; sensor logic does not create a second face or a second widget.

### Goals

- Detect the official right-slot Interaction module and react to insertion and removal.
- React to IMU shake bursts with the user-defined panic, confused and angry sequences.
- React to screen petting and poking on the ball area.
- React to LDR light changes and PIR presence.
- Use KEY_L and KEY_R as local emotional inputs.
- Use the vibration motor for reaction feedback.
- Include BMM150 approach, contact, release, edge and orbit events in the first version.
- Make 14 expressions reachable in the first version while retaining the 32-expression catalog in the engine.

### Non-goals

- Camera image processing; the left slot remains the camera slot.
- Voice/ASR/LLM implementation; voice consumes `Listening`, `Thinking`, `Speaking` and `Error` from a later service.
- Full tuning of all 32 expressions in the first release.
- Magnetic tile topology, ESP-NOW, or the full `mosaico_interaction` game layer.
- Replacing RFC 0003's renderer or RFC 0002's reconciler.
- Treating sensor reactions as safety-critical behavior.

## 2. Hardware truth

The official board configuration and BSP define the following hardware.

### 2.1 Main board

| Device | Interface | Use |
| --- | --- | --- |
| BMI270 | I2C0 `0x68` | Accel/gyro; shake, pickup, orientation |
| BMM150 `right` | I2C0 `0x22` | Magnetic field, approach/contact/orbit |
| BMM150 `left` | I2C0 `0x24` | Magnetic field, approach/contact/orbit |
| CST9220 | I2C0, INT `GPIO6` | Screen touch; petting/poking |
| `button_power` | `GPIO7` | Short PTT, long Hub |
| `button_boot` | `GPIO61` | Debug/stats; no expression |
| Vibration motor | `GPIO8` LEDC | Haptic patterns |

The main board has no ambient-light sensor and no PIR. Those functions come from the Interaction module.

### 2.2 Interaction module in the right slot

The official Interaction module is discovered through the slot EEPROM:

| Signal | Left canonical | Right slot actual | Notes |
| --- | --- | --- | --- |
| Slot EEPROM | `0x50` | `0x51` | Address select `GPIO39=1` |
| Board type | `0x16` | `0x16` | `MOSAICO_BOARD_TYPE_INTERACT` |
| Subboard I2C | SDA `GPIO0`, SCL `GPIO1` | Same bus | Separate from main I2C0 |
| LDR | `GPIO53` | `GPIO46` | ADC; filtered relative level 0–100 |
| IR TX | `GPIO48` | `GPIO47` | RMT NEC; not used by reactions |
| KEY_L | `GPIO13` | `GPIO11` | GPIO or SI12T touch |
| KEY_R | `GPIO12` | `GPIO10` | GPIO or SI12T touch |
| PIR | `GPIO4` | `GPIO5` | Digital input, pull-down |
| WS2812 | `GPIO15` | `GPIO38` | 6 LEDs; optional mood color |

The left slot EEPROM is `0x50` and belongs to the camera. The right slot EEPROM is `0x51` and belongs to the Interaction module.

`mosaico_module_mgr` scans slots every 250 ms with three debounce samples. RFC 0004 uses the same cadence and reports a stable `ModuleEvent` only after the debounce count.

### 2.3 Interaction module input semantics

The official `mosaico_interact_read_inputs` returns:

```c
typedef struct {
    bool left_pressed;
    bool right_pressed;
    bool motion_detected;
    int light_raw;
    uint8_t light_level; // 0-100, not lux
} mosaico_interact_inputs_t;
```

- `motion_detected` is a sampled boolean, not an edge event. SensorTask creates the rising and falling edges.
- `light_level` is a filtered relative level. AffectEngine uses baseline and rate of change, not absolute lux.
- KEY_L and KEY_R share pads with optional SI12T touch. The first version uses GPIO keys; SI12T touch is a follow-up.
- The module may be absent. IMU, touch, buttons and BMM150 continue to work.

### 2.4 Camera slot

The left slot is reserved for the camera. Camera insertion and removal produce a generic `Curious` reaction; the Interaction-specific reaction applies only to the right slot. Camera initialization and image processing remain outside this RFC.

## 3. Reaction layers

AffectEngine separates slow mood from fast reactions and external activity.

| Layer | Examples | Lifetime | Source |
| --- | --- | --- | --- |
| `Activity` | Listening, Thinking, Speaking, Error | Session-defined | Voice/LLM/C3 |
| `Reaction` | Panic, Angry, Happy, Puzzled, Confused, Surprised | 0.5–10 s | Sensors, touch, keys |
| `Mood` | energy, annoyance, delight, curiosity | Seconds to minutes | Slow sensor accumulation |
| `Idle` | Idle, Curious, Sleep | Until next event | Default |

Resolution order (rendering priority):

```text
Error > Panic > Angry (during its hold) > Physical reaction > Ambient reaction > Activity > Mood > Idle
```

- Activity is a persistent session state, not a high-priority transient. A short physical or ambient reaction overlays Activity, then returns to the active Activity state.
- `Panic` interrupts Activity and every lower layer.
- `Angry` interrupts Activity for its 10-second hold; after expiry, return to Activity if it is still active, otherwise to Idle.
- `Error` is the highest layer and interrupts Panic. It clears only when the external service reports that the error is resolved (or sends `Activity(Idle)`).
- Voice/LLM activity enters through the same event contract as `Activity(BallExpression)`; no second expression resolver is introduced.
- A module insertion/removal reaction overlays Activity briefly and returns.
- Only one expression renders at a time. Mood never directly selects an expression while a Reaction or Activity is active.

### 3.1 SensorTask

- IMU: 50–100 Hz; accel and gyro.
- BMM150: 20–50 Hz; official `mosaico_interaction` classifier.
- Interaction module: 20–50 Hz poll; LDR, PIR, keys.
- Screen touch: existing 5 ms sampling; gesture detection in the touch path.
- Buttons: event-driven or 20 ms poll.
- Module watcher: 250 ms EEPROM scan, three-sample debounce.

All samples carry `timestamp_ms` from the same monotonic clock. SensorTask publishes through a bounded channel; a full queue drops the oldest non-edge event and increments a counter.

### 3.2 AffectEngine

AffectEngine is pure Rust with no hardware or LVGL dependency. It exposes:

```rust
pub enum BallExpression {
    Idle, Sleep, Curious, Happy, Puzzled, Confused,
    Surprised, Panic, Angry, Satisfied,
    Listening, Thinking, Speaking, Error,
}

pub struct AffectState {
    pub expression: BallExpression,
    pub intensity: u8,
    pub since_ms: u64,
    pub reason: ReactionReason,
}

pub struct AffectEngine { /* timers, counters, mood */ }

impl AffectEngine {
    pub fn on_event(&mut self, event: SensorEvent);
    pub fn tick(&mut self, now_ms: u64) -> AffectTick;
}
```

`AffectTick` contains the selected expression, whether it changed, an optional `HapticPattern`, and the return deadline. The engine is deterministic; the host tests replay synthetic streams and compare timelines.

## 4. Event rules

### 4.1 IMU shake

Shake detection uses filtered acceleration magnitude and gyro magnitude. A burst is one shake episode followed by rest.

| Event | Rule | Reaction | Return |
| --- | --- | --- | --- |
| Large shake | Two peaks above the large threshold within 200 ms, or gyro above the large threshold | `Panic` for 1.5 s | No motion for 5 s → `Confused` 3 s → prior state |
| Small shake burst | 1–3 oscillations above the small threshold, then 300–800 ms rest | `Confused` 2.5 s | Prior state |
| Repeated small bursts | 5–10 bursts within 15 s, each separated by 300–800 ms rest | `Angry` 10 s | Prior state; 30 s angry cooldown |
| Continuous shake | No new burst while motion remains above rest threshold | Keep current Panic/Confused | Start rest timer only after motion falls below rest threshold |
| Pickup/flip | Gravity direction change plus gyro | `Surprised` 1.2 s | Prior state |

The large shake clears the small-burst counter. Continuous shaking does not increment the burst counter; the rest gap is required.

Initial thresholds are calibration points, not final values:

- Small dynamic acceleration: `0.3–0.8 g`.
- Large dynamic acceleration: above `1.2 g` or gyro above `200 °/s`.
- Rest: below `0.08 g` dynamic and `20 °/s` for 300 ms.
- Burst rest gap: 300–800 ms.
- Angry burst count: 5–10.
- Panic hold: 1.5 s; confusion after no motion: 5 s.

### 4.2 Screen touch

Touch input is restricted to the ball area, not the full screen.

| Event | Rule | Reaction | Return |
| --- | --- | --- | --- |
| Petting | At least two direction reversals, path >80 px, within 1.5 s | `Happy` while held | Release + 1.5 s → prior state |
| Single poke | Short press with <12 px movement | `Puzzled` 1.5 s | Prior state |
| Rapid pokes | Three pokes within 1.5 s | `Angry` 10 s | Prior state; shared angry cooldown |
| Long stroke | Slow one-direction stroke >150 px | `Satisfied` 1.5 s | Prior state |

Touch events use the existing `InputQueue` timestamps. The gesture detector runs in the input task and publishes `TouchGesture`; it does not block the UI task.

### 4.3 Light

`light_level` is relative 0–100. AffectEngine keeps a rolling baseline and reacts to deltas.

| Event | Rule | Reaction | Return |
| --- | --- | --- | --- |
| Sudden bright | +25 points or more within 300 ms | `Surprised` 1.5 s | Prior state |
| Sudden dark/cover | −25 points or more within 300 ms | `Confused` 2 s | Prior state |
| Gradual dawn | Slow rise over 30 s | `Curious` 2 s → `Idle` | Idle |
| Night | Level below 10 for 60 s | `Sleep` | Any touch, PIR or bright light wakes |

Absolute level does not select an expression. A slow daylight change must not trigger `Surprised`.

### 4.4 PIR

| Event | Rule | Reaction | Return |
| --- | --- | --- | --- |
| Presence rising | Debounced 0→1 | `Curious` 3 s | Idle |
| Presence sustained | 10 s continuous presence | `Satisfied` 1.5 s | Idle; no repeat within 30 s |
| Presence falling | Debounced 1→0 | No immediate reaction | 60 s absence → `Sleep` |

PIR pulses shorter than 200 ms are ignored. The sensor is sampled; the absence timer starts on the confirmed falling edge.

### 4.5 Keys and buttons

| Input | Action | Reaction | Haptic |
| --- | --- | --- | --- |
| KEY_L short | Soothe | `Happy` 1.5 s | Soft tick |
| KEY_L hold | Continued soothe | `Happy` while held | Soft tick on entry |
| KEY_R short | Poke | `Puzzled` 1.5 s | Double tick |
| KEY_R triple within 1.5 s | Repeated poke | `Angry` 10 s | Strong double pulse |
| KEY_L + KEY_R within 300 ms | Hug | `Satisfied` 2 s | Soft double tick |
| `button_power` short | Voice PTT | `Listening` | Short tick |
| `button_power` long 500 ms | Hub/drawer | No expression change | Long tick |
| `button_power` double | Stop/cancel | Stop the active session | Double tick |
| `button_boot` | Debug/stats | No expression change | None |

KEY_L and KEY_R are the right-slot `GPIO11` and `GPIO10` in GPIO mode. SI12T touch mode uses the same logical keys and is deferred until the GPIO path is stable.

### 4.6 BMM150

The official `mosaico_interaction` classifier produces approach, contact, release, edge-change, orbit CW/CCW and orientation events. The first version uses these events with confidence gating.

| Event | Condition | Reaction | Return |
| --- | --- | --- | --- |
| Approach | Classifier approach with confidence ≥ 0.5 | `Curious` 2 s | Prior state |
| Contact | Contact confirmed | `Satisfied` 1.5 s | Prior state |
| Release | Release confirmed | `Confused` 2 s | Prior state |
| Orbit CW/CCW | Rotation event with confidence ≥ 0.5 | `Surprised` 1.2 s | Prior state |
| Edge changed | Valid edge change | `Puzzled` 1.5 s | Prior state |
| Orientation changed | Stable orientation change | `Curious` 1.5 s | Prior state |

Saturated samples, low confidence, and calibration failures produce no expression. The official magnetic profile is a prototype; thresholds must be recalibrated after enclosure or magnet changes.

### 4.7 Module insertion and removal

| Event | Reaction | Haptic |
| --- | --- | --- |
| Right-slot Interaction inserted | `Surprised` 0.5 s → `Curious` 1.5 s → `Happy` 1.2 s → prior state | Single tick |
| Right-slot Interaction removed | `Surprised` 0.4 s → `Confused` 2 s → prior state | Double tick |
| Left-slot camera inserted | `Curious` 1.5 s → prior state | Single soft tick |
| Left-slot camera removed | `Confused` 1.5 s → prior state | Double soft tick |

Insertion and removal are derived from EEPROM presence and board type, not from the LDR/PIR pins. The module watcher reports a stable event after three consecutive scans. A wrong board type, an EEPROM read error, or an unreadable descriptor produces no expression and increments a diagnostic counter.

### 4.8 Future C3 and media inputs

C3 temperature, fan state, fault state and PC media state use the same `SensorEvent` channel or `Ui::post`. Proposed mappings:

| Source | Event | Reaction |
| --- | --- | --- |
| C3 | Over-temperature | `Tired` |
| C3 | Fault or TEC cutoff | `Error` |
| C3 | Cooling stable | `Satisfied` |
| PC media | Playing | `Happy` / `Satisfied` |
| PC media | Paused | Idle |
| Voice | Listening / Thinking / Speaking / Error | Activity states |

These sources are outside the first sensor implementation; the event contract is defined so they do not require a second expression resolver.

## 5. Expression catalog

The grok-ball data contains 32 expressions in three groups:

| Group | IDs | Expressions |
| --- | --- | --- |
| Life | `00–07` | Sleep, Wake, Idle, Curious, Bored, Loading Wake, Dormant, Shake Wake |
| Emotion | `10–21` | Happy, Puzzled, Sad, Surprised, Shy, Tired, Focused, Panic, Helpless, Satisfied, Confused, Angry |
| Agent | `30–41` | Thinking, Receiving, Busy, Done, Error, Waiting, Loading, Recall, Refused, Speaking, Searching, Stopped |

The first version exposes 14:

`00 Sleep`, `02 Idle`, `03 Curious`, `10 Happy`, `11 Puzzled`, `13 Surprised`, `17 Panic`, `19 Satisfied`, `20 Confused`, `21 Angry`, `30 Thinking`, `34 Error`, `35 Listening`, `39 Speaking`.

The other 18 remain data-only until their reactions are tuned. A new expression is reachable only after a rule in section 4 and a host timeline test exist.

## 6. Haptic feedback

The vibration motor is on `GPIO8` through LEDC. Haptics are driven by reaction transitions, never by per-frame animation.

| Reaction/Event | Pattern | Intensity |
| --- | --- | --- |
| `Panic` | 3 × 80 ms, 60 ms gaps | 100% |
| `Angry` | 2 × 160 ms, 100 ms gap | 90% |
| `Surprised` | 1 × 50 ms | 70% |
| `Puzzled` | 2 × 40 ms, 50 ms gap | 60% |
| `Happy` | 1 × 60 ms | 50% |
| `Satisfied` | 1 × 70 ms | 55% |
| `Confused` | 2 × 35 ms, 60 ms gap | 45% |
| `Listening` | 1 × 50 ms | 60% |
| `Thinking` | 1 × 30 ms on entry | 35% |
| `Error` | 3 × 60 ms, 40 ms gaps | 100% |
| Module insert | 1 × 40 ms | 45% |
| Module remove | 2 × 40 ms, 50 ms gap | 45% |
| BMM150 contact | 1 × 30 ms | 35% |

Rules:

- Haptic patterns run in a dedicated haptic task or a non-blocking scheduler; the UI task never sleeps for a pattern.
- A new pattern replaces a pending pattern; `Panic` and `Error` have priority.
- Haptics are rate-limited to one pattern per 150 ms.
- No haptic for gaze, blink, breathing, or ordinary idle expression changes.
- The motor pattern and the expression transition start from the same `AffectTick`.
- WS2812 mood colors are optional; the first version uses the motor only.

## 7. Module lifecycle

1. `ModuleWatcher` probes right-slot EEPROM `0x51` and left-slot EEPROM `0x50` every 250 ms.
2. Three consecutive identical presence results produce a stable event.
3. A right-slot descriptor with `board_type == 0x16` enables the Interaction input source.
4. The watcher configures right-slot pins: LDR `GPIO46`, PIR `GPIO5`, KEY_L `GPIO11`, KEY_R `GPIO10`.
5. Insertion publishes `ModuleEvent::InteractionPresent`; removal publishes `ModuleEvent::InteractionAbsent` and disables LDR/PIR/KEY sampling.
6. A camera descriptor in the left slot publishes `ModuleEvent::CameraPresent/Absent`; image capture remains outside this plan.
7. Module absence is not a fault. IMU, BMM150, touch and buttons continue.
8. A module read error is logged and counted; it does not trigger an expression.

The right-slot address select is `GPIO39=1`. The subboard I2C bus is SDA `GPIO0`, SCL `GPIO1`. The main I2C0 devices remain on SDA `GPIO56`, SCL `GPIO3`.

## 8. Testing and acceptance

### 8.1 Host simulation

`AffectEngine` runs against recorded and synthetic `SensorEvent` streams:

- Large shake → Panic 1.5 s → 5 s still → Confused 3 s → Idle.
- Small shake → Confused 2.5 s.
- 5, 7 and 10 small bursts with valid rest gaps → Angry 10 s; four bursts → no Angry.
- Continuous shaking for 3 s → one burst only; no Angry count.
- Petting → Happy while held; release +1.5 s → prior state.
- Three pokes within 1.5 s → Angry.
- LDR bright/dark deltas; gradual daylight does not trigger Surprised.
- PIR rising/sustained/falling; absence timer.
- KEY_L/KEY_R short, triple, both keys.
- BMM150 approach/contact/release/orbit with low-confidence and saturated cases.
- Module insert/remove with bounce; wrong board type; missing EEPROM.
- Haptic pattern selection and replacement order.

Each case asserts the expression timeline, start/end times, return state, haptic pattern and cooldown counters.

### 8.2 Device acceptance after R1

- Right-slot Interaction module is detected; insertion and removal produce the specified expressions and haptics.
- LDR cover/uncover produces Confused/Surprised with no false trigger from slow light changes.
- PIR presence produces Curious/Satisfied; absence produces Sleep after 60 s.
- IMU large/small/repeated shake sequences match the host timelines.
- Petting on the ball area produces Happy; poking produces Puzzled/Angry.
- BMM150 approach/contact/release/orbit produce the mapped expressions; low-confidence samples are ignored.
- Haptic patterns match the table; no UI stalls.
- Module removal does not crash or leave stale sensor events.
- Voice Activity overlays and returns correctly.

### 8.3 Stop conditions

- Shake thresholds produce false Panic during normal desk handling; stop and recalibrate.
- Module detection misses or bounces; stop and match the official 250 ms/3-sample rule.
- Haptic patterns block the UI task or disturb I2C/display; stop and move to a scheduler.
- BMM150 confidence is unstable after calibration; disable BMM150 reactions and keep the rest.
- Angry cooldown or return states fail; stop before adding more expressions.

## 9. Milestones and estimates

| Wave | Work package | Deliverable | Estimate |
| --- | --- | --- | --- |
| H0.1 | Event schema and host simulator | `SensorEvent`, `AffectState`, replayed streams, timeline tests | 3–4 days |
| H0.2 | Shake and touch reaction engine | Burst segmentation, panic/confused/angry, petting/poke | 4–6 days |
| H0.3 | Ambient and keys | LDR, PIR, KEY_L/KEY_R, button mapping | 3–4 days |
| H1.1 | Interaction module driver | Right-slot EEPROM detection, LDR/PIR/keys, hot-plug | 4–6 days |
| H1.2 | BMM150 classifier integration | Approach/contact/release/orbit, confidence gating | 3–5 days |
| H1.3 | Haptics | LEDC patterns, priority and scheduler | 1–2 days |
| H2.1 | Device integration | SensorTask on R1, module hot-plug, expression timelines | 5–8 days |
| H2.2 | Calibration and acceptance | Shake/light/PIR/BMM150 thresholds, final timelines | 5–8 days |

Host work H0 can start in parallel with R0/R1. Device work H1/H2 follows R1. First sensor-enabled ball estimate: **4–6 weeks** after R1, with the host engine beginning earlier.

## 10. Risks and open decisions

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Interaction module absent or in the left slot | No LDR/PIR/keys | Right-slot EEPROM detection; disable inputs gracefully; camera left |
| Custom module with different pins | Wrong sensor reads | Require pin map; do not infer from the official module |
| Shake threshold false positives | Random Panic/Angry | Rest segmentation, gyro confirmation, calibration |
| Continuous shake counted as many | Wrong Angry | Burst/rest state machine; host timeline tests |
| PIR false triggers | Random Curious/Satisfied | 200 ms debounce, sustained presence, absence timer |
| LDR is relative, not lux | Absolute thresholds fail | Baseline/delta logic; 0–100 contract |
| BMM150 calibration drift | Wrong approach/contact | Confidence gate; disable on saturation; recalibrate |
| Haptic motor EMI or blocking | Display/I2C disturbance | Non-blocking scheduler; measure; rate limit |
| Reaction storms | Expression thrash | Priority, cooldown, max one reaction per 150 ms |
| KEY touch/GPIO sharing | Missed keys | GPIO first; SI12T touch follow-up; AUTO mode semantics |
| Module hot-plug race | Stale events | Three-sample debounce; generation counter; disable on removal |
| 32-expression tuning | Delay first screen | 14 reachable first; remaining data-only |

### Open decisions

| ID | State | Decision | Stop condition |
| --- | --- | --- | --- |
| D-REACT-01 | `CONFIRMED` | Official Interaction module in the right slot; camera left | Custom module requires a new pin map |
| D-REACT-02 | `CONFIRMED` | KEY_L soothe/Happy, KEY_R poke/Puzzled, triple poke Angry, both keys Satisfied | User changes key semantics |
| D-REACT-03 | `CONFIRMED` | BMM150 approach/contact/release/orbit in first version | Calibration cannot meet confidence gate |
| D-REACT-04 | `CONFIRMED` | Haptics on GPIO8 with the table in section 6 | Motor cannot produce distinct patterns |
| D-REACT-05 | `CONFIRMED` | LDR relative 0–100 | Absolute lux becomes a hard requirement |
| D-REACT-06 | `UNKNOWN` | Right-slot module is the official `0x16` board, not a custom board | Descriptor or pin map differs |
| D-REACT-07 | `UNKNOWN` | GPIO keys first; SI12T touch later | Touch pads are the only populated input |
| D-REACT-08 | `UNKNOWN` | Module insert/remove overlays an active voice session for its short sequence | Product requires no overlay during voice |

## 11. Handoff

### 11.1 To RFC 0003

RFC 0003's `BallWidget` renders the `BallExpression` selected by AffectEngine. Haptic patterns are separate from the canvas; the ball's `Ui::tick` does not drive the motor.

### 11.2 To R1

R1's sensor task handles BMI270, BMM150, the Interaction module, buttons and touch gesture publication. It publishes `SensorEvent` through a bounded channel. It never calls LVGL or the ball renderer.

### 11.3 To C3 and voice

C3 temperature/fan/fault and PC media events use the same event contract. Voice Activity sets `Listening`, `Thinking`, `Speaking` or `Error` and respects the priority rules.

### 11.4 Definition of done

- Host timeline tests pass for all rules in section 4.
- Right-slot Interaction module detection and hot-plug pass on device.
- 14 expressions are reachable; 32 remain in the catalog.
- Haptic patterns match section 6 without blocking the UI.
- BMM150 reactions pass confidence and calibration checks.
- Device acceptance in section 8.2 passes after R1.
