# Third-party sources

Everything under `vendor/` is copied from upstream. Only the changes listed
here were made.

## Lua 5.5.1 (`lua-5.5.1/`)

- Source: https://www.lua.org/ftp/lua-5.5.1.tar.gz
- License: MIT (`lua-5.5.1/LICENSE`)
- Unmodified. `loslib.c` is compiled through `csrc/mosaico_loslib.c`, which
  redirects `time`, `clock`, `system`, `tmpnam` and `exit` to the host with
  macros before including it (`remove`/`rename` go to the file system through
  newlib). `liolib.c` is compiled as is; its `FILE *` calls reach the
  `mosaico-fs` VFS through newlib's syscalls.

## esp-claw (`esp-claw/`)

- Source: https://github.com/espressif/esp-claw, commit
  `345073de68179542c307e303df81e38baadf77f0`
- License: Apache-2.0 (`esp-claw/LICENSE`)
- Files:
  - `lua_module_display/` (all sources, `README.md`, `test/display_contract.lua`)
  - `esp_painter/esp_painter_font.h`, `esp_painter/basic_font_{12..48}.c`
    (ten sizes; the font data is CC0, as stated in the upstream files)
  - `lua_module_json/`
  - `lua_module_system/arg_schema.lua`
  - `lua_module_storage/` (`lua_module_storage.c/.h`, `README.md`), unmodified;
    `claw_paths_get` comes from the stub `csrc/esp_shim/claw_paths.h` and
    returns the data root (`/data` on the device)
- Modification: `lua_module_display/lua_module_display.c` includes
  `mosaico_platform.h`, and `present` returns
  `mosaico_display_presented(L, updated)` so the script yields until the frame
  is on screen.
- Modification: `lua_module_display/display_text.c` `builtin_font()` takes the
  requested size and returns the builtin font of that height (else the next
  larger, else the largest). Upstream compiles one font chosen in Kconfig and
  scales it, which blurs every size but one. All builtin fonts are h/2 wide, so
  text metrics are unchanged. Nothing else changed. The ESP-IDF headers it needs are stubs in
  `csrc/esp_shim/`, backed by `csrc/mosaico_platform.c`.

## esp-claw-skills-lab (`skills-lab/`)

- Source: https://github.com/espressif/esp-claw-skills-lab, commit
  `57969b1e5891b5ad68e3d2433f1e9dde3ab1fcc0`
- License: MIT (`skills-lab/LICENSE`)
- Files: `snake/`, `game_2048/`, unmodified.

## cJSON (`cjson/`)

- Source: https://github.com/DaveGamble/cJSON, v1.7.18
- License: MIT (`cjson/LICENSE`)
- Files: `cJSON.c`, `cJSON.h`, unmodified.

## TJpgDec R0.03 (`tjpgd-r0.03/`)

- Source: LVGL 9.6 `src/libs/tjpgd` (via lvgl_binding_rust `cad0311`),
  originally by ChaN
- License: ChaN's BSD-style terms in the header of `tjpgd.c`
- Modification: `tjpgd.h` includes `<stdint.h>` and defines `LV_USE_TJPGD`
  instead of LVGL's header (see `tjpgd-r0.03/README.md`). Used to decode the
  launcher icons (`assets/icon.jpg`).

## littlefs 2.11.3 (`crates/mosaico-fs/vendor/littlefs-2.11.3/`)

- Source: https://github.com/littlefs-project/littlefs, v2.11.3
- License: BSD-3-Clause (`LICENSE.md` there)
- Files: `lfs.c`, `lfs.h`, `lfs_util.c`, `lfs_util.h`, unmodified; the
  configuration is passed as defines from `crates/mosaico-fs/build.rs`.
