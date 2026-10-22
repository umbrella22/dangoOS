# ESP-Mosaico S31：狐狸团子

ESP-Mosaico v1.2 显示终端（ESP32-S31，CO5300 480×480 QSPI AMOLED，CST9220 触摸，
BMI270 / BMM150，BQ27220）上的 Rust 固件。主页是“狐狸团子”：一只调色板精灵狐狸，
表情跟着触摸、晃动、光线、磁铁、左右扩展模块和电量变化。从主页唤出应用网格，
里面有原生应用（设置 / 传感器 / 计时器）和兼容官方格式的 Lua 5.5 应用，
Lua 应用可以从电脑经 USB 推送并热重载。

**接手先读 [docs/HANDOFF.md](docs/HANDOFF.md)**：现状、下一步、后续拓展和文档地图都在那里。

## 结构

| crate | 作用 |
| --- | --- |
| `crates/mosaico-s31` | 库（`behavior` / `fox` / `hub` / `ui` / `telemetry`，host 可测）+ 固件（`drivers` / `tasks`） |
| `crates/mosaico-lua` | Lua 5.5.1 与官方 esp-claw 模块移植层 |
| `crates/mosaico-fs` | littlefs + VFS，`/system` 与 `/data` |

完整目录见 [docs/PROJECT_STRUCTURE.md](docs/PROJECT_STRUCTURE.md)，狐狸从 PNG 到屏幕见
[docs/FOX_RENDER_PIPELINE.md](docs/FOX_RENDER_PIPELINE.md)。素材和 Python 管线在 `assets/fox-dango/`。
UI 层是 [lvgl_binding_rust](https://github.com/umbrella22/lvgl_binding_rust) 的
[`lvgl-ui`](https://github.com/umbrella22/lvgl_binding_rust/blob/master/lvgl-ui/README.md)。

## 常用命令

```bash
./scripts/check.sh                                   # 不需要板子的全部检查（测试、预览、固件、ROM 符号）
cargo fw --offline                                   # 固件 release 构建
./scripts/flash-app.sh /dev/cu.usbmodemXXXX          # 下载模式下只写 main_app（0x210000）
python3 scripts/push-app.py push examples/lua-apps/hello_push --run --follow   # 推送 Lua 应用
```

细节见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 烧录红线

只写 `main_app`（`0x210000`）。**不要**改写 bootloader（S31 上在 `0x2000`）、分区表（`0x8000`）
或 `0x20000` 的出厂 `vibe_mode`。

## 构建环境

- `esp-hal`、`esp-bootloader-esp-idf`、`esp-alloc`、`esp-rtos`、`esp-storage` 固定在 esp-hal 仓库的同一个
  Git revision（S31 支持还没进正式版），升级时一起改并真机验证。
- `lvgl` / `lvgl-ui` 是 Git 依赖，`rev` 固定在根 `Cargo.toml`。想试本地 checkout，
  在自己的 `.cargo/config.toml` 里加 `[patch."https://github.com/umbrella22/lvgl_binding_rust"]`，不要提交。
- `.cargo/config.toml` 为作者的机器设置了 Espressif C 工具链（`CC_…`、`AR_…`）、`LIBCLANG_PATH`
  和 `LVGL_SYSROOT`。Cargo 的 `[env]` 不覆盖已设置的变量，换机器时在 shell 里导出自己的值即可：

  ```bash
  export ESP_TOOLS=$HOME/esp-tools/riscv32-esp-elf
  export CC_riscv32imafc_unknown_none_elf=$ESP_TOOLS/bin/riscv32-esp-elf-gcc
  export AR_riscv32imafc_unknown_none_elf=$ESP_TOOLS/bin/riscv32-esp-elf-ar
  export LVGL_SYSROOT=$ESP_TOOLS/riscv32-esp-elf
  ```

## 控制台

USB-OTG HS CDC-ACM（不是 USB-Serial-JTAG），运行时枚举为 `/dev/cu.usbmodemmosaico_s31_r11`。
同一个口既出日志也收推送命令。统计行的含义见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。
