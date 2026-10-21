# Mosaico UI layer — LVGL widgets demo compatibility report

> Status: observed on 2026-10-20 against vendored LVGL 9.6.0.
> Replica: [`lvgl-ui/examples/widgets_demo.rs`](../../../lv_binding_rust/lvgl-ui/examples/widgets_demo.rs).
> Purpose: exercise the typed layer against LVGL's official `widgets` demo and
> record which components and behaviors adapt directly, which use the raw
> escape, and which are missing.

## Method

The official demo (`lvgl-sys/vendor/lvgl-9.6/demos/widgets/`) is a three-tab
screen: Profile, Analytics, Shop. The replica keeps the same tab content and
uses the same widgets where the typed layer supports them. Missing capabilities
found during the exercise were added to `lvgl-ui`; only the calendar still uses
`unsafe Node::raw`.

Run it with:

```sh
cargo run -p lvgl-ui --example widgets_demo --features simulator
```

Useful environment variables:

| Variable | Meaning |
| --- | --- |
| `DEMO_FRAMES=<n>` | Stop after `n` frames (bounded smoke run). |
| `DEMO_TAB=profile\|analytics\|shop` | Start on one tab. |
| `DEMO_SCROLL_Y=<px>` | Scroll the active tab page before the screenshot. |
| `DEMO_KEYBOARD=<id>` | Start with the keyboard bound to textarea `id` visible. |
| `DEMO_SCREENSHOT=<path>` | Write a PPM snapshot of the active screen. |

## Coverage matrix

| Official demo element | Replica implementation | Status |
| --- | --- | --- |
| `lv_obj` / flex containers | `Node::obj` / `Node::row` / `Node::column` | Typed |
| `lv_label` | `Node::label` | Typed |
| `lv_button` | `Node::button` | Typed |
| `lv_slider` | `Node::slider` | Typed |
| `lv_switch` | `Node::switch` | Typed |
| `lv_checkbox` | `Node::checkbox` + `on_value_changed` | Typed |
| `lv_textarea` (name/password/birthday) | `Node::textarea` + `on_text_changed` | Typed |
| `lv_dropdown` | `Node::dropdown` + `on_value_changed` | Typed |
| `lv_arc` (gauges) | `Node::arc` with `Style::arc_color`/`arc_width` | Typed |
| `lv_scale` | `Node::scale` (basic linear) | Typed (basic) |
| `lv_chart` (line/bar/stacked) | `Node::chart` with `ChartSeries`/`ChartType` | Typed |
| Theme/accent switching | `Theme` override + palette buttons + `Ui::set_theme` | Typed |
| `lv_tabview` | `Node::tabview`; one child per tab page; `on_value_changed` reports the active index | Typed |
| Grid layout (`lv_obj_set_grid_cell`) | `Node::grid`, `GridTrack`, `GridCell`, `grid_align` | Typed |
| `lv_calendar` | `Node::raw` with `lv_calendar_*` | Raw |
| `lv_keyboard` | `Node::keyboard(target)` + textarea `keyboard_id(target)` | Typed |
| `lv_image` (avatar, clothes, logo) | `ImageSource::rgb565` with a generated static image; official assets are not bound | Typed source (assets gap) |
| Color-changer animation/palette | Typed palette buttons; no open/close animation | Partially adapted |
| Chart/scale animations | Static values | Gap (animation objects) |
| Disabled button/checkbox states | `Style::disabled` / `Style::hidden` | Typed |
| Part-specific styles (arc indicator/knob) | `Node::part_style(Part, Style)`; `Style::arc_color`/`arc_width` for the indicator | Typed |
| Animation duration style | `Style::anim_duration` | Typed |
| Focus/keyboard/textarea integration | Textarea `keyboard_id`, `on_event` Focused/Defocused/Ready/Cancel, keyboard binding pass | Typed |

## Findings

1. **The typed core now covers the demo's layout and widget set.** Profile,
   Analytics and Shop use typed layout, grid, tabview, labels, buttons,
   sliders, switches, checkboxes, textareas, dropdowns, keyboards, arcs, scales,
   charts and RGB565 images. Only the calendar remains raw.
2. **Raw escape is now limited to the calendar.** `Node::raw` factories create
   static widgets; the reconciler does not inspect their children or internal
   state. `Node::style` is ignored for raw nodes, so raw factories must apply
   their own styles. Theme changes do not restyle raw content.
3. **Grid layout is typed.** `Node::grid` builds column/row templates and
   `grid_cell`/`grid_align` place children. The profile form now uses a real
   two-column grid. The default cell alignment fills columns and starts rows,
   which avoids a Content-row sizing cycle with stretch children.
4. **The image API is safe for static RGB565 data.** `ImageSource::rgb565`
   builds and owns the LVGL descriptor in the mounted tree. The official demo's
   `lv_image_dsc_t` assets are still not declared in a header bindgen parses, so
   the replica uses a generated placeholder image; an asset pipeline/decoder
   remains future work.
5. **`Style` now covers the remaining form, part and transition needs.**
   Per-side padding, text alignment, min/max sizes, hidden/disabled state, arc
   indicator color/width and `anim_duration` are typed. `Node::part_style`
   targets scrollbar/indicator/knob/selected/items/cursor parts. Animation
   objects themselves remain app-level.
6. **Message flow and focus are typed across the new widgets.** Checkbox,
   textarea, dropdown, arc and tabview events use
   `on_value_changed`/`on_text_changed`; textarea focus and keyboard
   Ready/Cancel use `on_event`. A post-render binding pass resolves
   `keyboard_id` targets so creation order does not matter. The implementation
   also fixed a suppression bug where a non-value event during a textarea write
   could consume the flag meant for the following value event.

## Remaining gaps

1. Official image asset/decoder pipeline (the safe RGB565 source exists, but the
   demo assets themselves are not bound).
2. Animation object wrappers (style durations are typed; scripted animations
   are still platform/raw code).

The calendar remains the only raw widget in the replica.

## Verification

| Command | Result |
| --- | --- |
| `cargo build -p lvgl-ui --example widgets_demo --features simulator` | exit 0 |
| `DEMO_FRAMES=240 cargo run -p lvgl-ui --example widgets_demo --features simulator` | exit 0; 142 nodes mounted (all three tab pages); clean teardown |
| `DEMO_TAB=analytics DEMO_FRAMES=60 ...` | exit 0; typed charts, arcs, scale and checkboxes render in the native tabview |
| `DEMO_TAB=shop DEMO_FRAMES=60 ...` | exit 0; typed stacked chart, RGB565 product images and typed notifications render |
| `DEMO_TAB=profile DEMO_SCROLL_Y=600 DEMO_SCREENSHOT=...` | exit 0; grid form, one-line inputs, calendar and part-styled slider render; the tab page scrolls to the skills card |
| `DEMO_TAB=profile DEMO_KEYBOARD=1 DEMO_SCREENSHOT=...` | exit 0; 143 nodes; keyboard renders and is bound to textarea id 1 |
| `cargo test -p lvgl-ui -- --test-threads=1` | 41 tests, three consecutive passes |
| `cargo test -p lvgl-ui --no-default-features --features alloc -- --test-threads=1` | 41 tests pass without the simulator feature |
| `cargo build -p lvgl-ui --target riscv32imafc-unknown-none-elf --no-default-features --features alloc,no-autoinit` | exit 0 |

Visual snapshots were inspected during development. They are generated artifacts
and are not committed.

## Limitations

- This is a behavioral/structure replica, not a pixel-perfect port. Grid
  breakpoints, exact assets, animations and focus/keyboard flows differ.
- The replica runs on the host SDL2 backend. It does not measure hardware touch
  latency or frame time.
- Raw widgets in the example are static; the official demo's animated charts and
  scrolling slideshow are not reproduced.
