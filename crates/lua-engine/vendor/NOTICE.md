# 第三方源码与本地修改

此 `vendor/` 下的内容来自上游，源码修改列在下文。说明性 Markdown 已在本仓库翻译成中文；许可证原文、代码中的版权声明、上游提交号和原始来源哈希保留。中文 README 属于本地文档修订，不再声明与上游文档逐字节一致。

## Lua 5.5.1（`lua-5.5.1/`）

- 来源：<https://www.lua.org/ftp/lua-5.5.1.tar.gz>。
- 许可证：MIT，见 `lua-5.5.1/LICENSE`。
- 修改：`lstate.h`、`lstate.c`、`lgc.c` 增加可选的受限 VM 标志，垃圾回收和关闭时跳过脚本 `__gc`，丢弃时关闭上值但不运行 `__close`。标志默认值为零，普通前台 VM 终结器行为不变。
- `loslib.c` 通过 `csrc/mosaico_loslib.c` 编译；包含前用宏将 `time`、`clock`、`system`、`tmpnam`、`exit` 重定向到宿主，`remove`/`rename` 通过 newlib 进入文件系统。`liolib.c` 原样编译，其 `FILE *` 调用通过 newlib 系统调用进入 `fs` VFS。

## esp-claw（`esp-claw/`）

- 来源：<https://github.com/espressif/esp-claw>，提交 `345073de68179542c307e303df81e38baadf77f0`。
- 许可证：Apache-2.0，见 `esp-claw/LICENSE`。
- 导入文件：`lua_module_display/` 全部源码、README 和 `test/display_contract.lua`；`esp_painter/esp_painter_font.h` 和 10 种尺寸的 `esp_painter/basic_font_{12..48}.c`；`lua_module_json/`；`lua_module_system/arg_schema.lua`。字体数据按上游文件声明使用 CC0。
- `hardware-contracts/{imu,magnetometer,fuel_gauge,led_strip,button}/` 保存同一提交的上游 API 文档、未修改的测试脚本及 `lib_fuel_gauge.lua`，作为接口依据与兼容夹具。文档已中文化，上游 GPIO/I2C 驱动不参与构建。`csrc/mosaico_hardware.*` 是项目自有的传感器快照适配器。`fuel_gauge` 也提供上游 `lib_fuel_gauge` 的 require 名称；本项目不支持外部总线。该上游版本没有 haptic 模块，本项目 `haptic` 为开发板预设扩展。
- `lua_module_storage/` 中 `lua_module_storage.c/.h` 源码不变，README 已中文化。`claw_paths_get` 由 `csrc/esp_shim/claw_paths.h` 适配，返回当前 VM 的数据根目录；前台应用为 `/data/appdata/<id>`，缺少 VM 上下文时回退 `/data`。
- 显示模块修改：`lua_module_display/lua_module_display.c` 包含 `mosaico_platform.h`，`present` 返回 `mosaico_display_presented(L, updated)`，使脚本让出执行直到帧上屏。
- 字体修改：`lua_module_display/display_text.c` 的 `builtin_font()` 按请求高度返回对应内置字体，否则选择下一个更大尺寸，再否则选择最大尺寸。上游仅编译 Kconfig 选中的单种字体并缩放，除原尺寸外会模糊。全部内置字形宽度为 h/2，因此文本度量不变。除上述修改及文档翻译外，其余内容保留。所需 ESP-IDF 头部由 `csrc/esp_shim/` 适配，底层实现为 `csrc/mosaico_platform.c`。

## cJSON（`cjson/`）

- 来源：<https://github.com/DaveGamble/cJSON>，v1.7.18。
- 许可证：MIT，见 `cjson/LICENSE`。
- 文件：`cJSON.c`、`cJSON.h`，未修改。

## TJpgDec R0.03（`tjpgd-r0.03/`）

- 来源：LVGL 9.6 的 `src/libs/tjpgd`，通过 lvgl_binding_rust `cad0311` 导入，原作者 ChaN。
- 许可证：`tjpgd.c` 头部中的 ChaN BSD 风格条款。
- 修改：`tjpgd.h` 包含 `<stdint.h>` 并定义 `LV_USE_TJPGD`，替代 LVGL 头部，详见 [TJpgDec 说明](tjpgd-r0.03/README.md)。用于解码启动器图标 `assets/icon.jpg`。

## littlefs 2.11.3（`crates/fs/vendor/littlefs-2.11.3/`）

- 来源：<https://github.com/littlefs-project/littlefs>，v2.11.3。
- 许可证：BSD-3-Clause，见对应目录的 `LICENSE.md`。
- `lfs.c`、`lfs.h`、`lfs_util.c`、`lfs_util.h` 未修改，配置通过 `crates/fs/build.rs` 的定义传入。README 为本地中文译文。

[LodePNG](lodepng/README.md) 和 [stb_image_write](stb/README.md) 的来源与编译适配保存在各自目录。应用来源和许可证随各应用保存在仓库的 `apps/` 下。
