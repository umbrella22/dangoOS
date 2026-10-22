TJpgDec R0.03 (C) ChaN, 2021, as shipped in LVGL 9.6 `src/libs/tjpgd`
(lvgl_binding_rust cad0311). License terms are in the header of `tjpgd.c`.

Local change: `tjpgd.h` includes `<stdint.h>` and defines `LV_USE_TJPGD`
instead of including LVGL's public header, so it builds outside LVGL.

LVGL's copy writes RGB888 pixels in B, G, R byte order; `src/jpeg.rs`
swaps them to R, G, B.

`build.rs` renames `jd_prepare`/`jd_decomp` to `mosaico_jd_*`: the ESP32-S31
ROM has its own TJpgDec with a different `JDEC` layout, and the esp-hal ROM
linker script would otherwise resolve the calls to it.
