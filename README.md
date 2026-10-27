# ESP-Mosaico：狐狸团子

面向 ESP-Mosaico V1.2 的 Rust 系统，使用 ESP32-S31、CO5300 480×480 QSPI AMOLED、CST9220 触摸、BMI270 / BMM150 和 BQ27220。主页“狐狸团子”的表情随触摸、晃动、光线、磁铁、左右扩展模块和电量变化。启动器提供原生设置、传感器、计时器，以及通过文件系统加载的 Lua 5.5 应用。

**阅读与学习从[文档总入口](docs/README.md)开始。** [当前状态](docs/guides/current-status.md)记录现有边界与最近一次设备验证；[开发指南](docs/guides/development.md)提供环境、测试、构建和控制台命令。

## 目录与职责

| 位置 | 职责 |
| --- | --- |
| `crates/os` | 系统库、硬件驱动、任务调度、原生页面和 `os-fw` 固件 |
| `crates/fs` | littlefs、VFS、`/system` 与 `/data` |
| `crates/lua-engine` | Lua 5.5.1、esp-claw 模块与宿主适配 |
| `apps/games`、`apps/tools` | Snake、2048、Dino、Flappy、Photo 的独立源码 |
| `tools/app-pack` | 应用校验、完整 system 镜像构建与提取 |
| `examples`、`tests`、`scripts` | 开发示例、回归/设备验收、检查/设备/素材工具 |
| `docs` | 按指南、架构、硬件、素材、OTA、测试、排障、参考与历史分类的中文文档 |

完整流程见[源码结构](docs/architecture/project-structure.md)，应用约定见 [Lua 应用开发](docs/guides/lua-apps.md)，狐狸从 PNG 到屏幕的过程见[渲染管线](docs/assets/fox-rendering.md)。UI 使用 crates.io 发布的 [rs_lvgl](https://crates.io/crates/rs_lvgl) 和 [rs_lvgl_ui](https://crates.io/crates/rs_lvgl_ui)。

## 常用命令

所有命令从仓库根目录执行：

```sh
./scripts/check.sh                              # 统一离线检查，不连接设备
cargo fw --offline                              # 固件 release 构建
cargo run -p app-pack --offline -- check apps     # 校验正式应用
cargo run -p app-pack --offline -- system apps target/system-release --size 3072000
python3 scripts/device/console.py push examples/apps/hello --run --follow
python3 scripts/device/console.py ota-status      # 查看运行槽、自检和备用槽
```

system 输出目录必须是新目录，大小与目标设备分区一致。固件镜像序列化及 OTA 命令见 [USB 更新指南](docs/ota/usb-update.md)。

## 固件与应用更新

Rust 原生应用随固件构建。五个 Lua 应用统一由 system 镜像交付，可通过 USB 下载到 `/data/apps/<id>` 单独更新并覆盖系统版本；移除下载版后恢复 `/system/apps/<id>`。照片和存档位于 `/data/appdata/<id>`，不打包进 system 镜像。

固件采用 A/B 交替更新：A 运行时写 B，B 运行时写 A。写入完整校验后切换启动，候选通过健康窗口才标记 `Valid`，上一版保留在原运行槽。system/data 由两槽共用，固件回退不回退 Lua 文件和用户数据。

正常升级不写引导程序、分区表或出厂分区。ROM 下载模式恢复和旧布局迁移属于独立流程，见[更新与恢复](docs/ota/README.md)；仅写槽 0 的恢复脚本不会改变 OTA 启动选择。

## 构建环境

- `esp-hal`、`esp-bootloader-esp-idf`、`esp-alloc`、`esp-rtos`、`esp-storage` 固定在 esp-hal 仓库同一提交。芯片支持与依赖升级需一起核对并实机验证。
- `lvgl` / `lvgl-ui` 分别是 crates.io 包 `rs_lvgl` / `rs_lvgl_ui` 的依赖别名，版本要求为 `0.2.0`，精确版本由 `Cargo.lock` 锁定。控件与字体按需启用，升级时一起核对。临时使用本地检出可在本机 `.cargo/config.toml` 的 `[patch.crates-io]` 下配置实际包名与路径，详见[开发指南](docs/guides/development.md)。
- `.cargo/config.toml` 提供作者机器的 Espressif C 工具链、`LIBCLANG_PATH` 和 `LVGL_SYSROOT`。Cargo `[env]` 不覆盖已设置变量，其他机器可在 shell 导出自己的路径：

```sh
export ESP_TOOLS=$HOME/esp-tools/riscv32-esp-elf
export CC_riscv32imafc_unknown_none_elf=$ESP_TOOLS/bin/riscv32-esp-elf-gcc
export AR_riscv32imafc_unknown_none_elf=$ESP_TOOLS/bin/riscv32-esp-elf-ar
export LVGL_SYSROOT=$ESP_TOOLS/riscv32-esp-elf
```

## 控制台与应用生命周期

运行时控制台使用 USB-OTG HS CDC-ACM，在当前 macOS 环境通常枚举为 `/dev/cu.usbmodemmosaico_s31_r11`，同一端口提供日志与命令。统计含义见[开发指南](docs/guides/development.md)。

启动器短按打开应用、长按确认删除或清除数据。`scripts/device/console.py rm <id>` 删除下载应用，`rmdata <id>` 清除应用数据；成功回执表示停止并销毁 VM 后的实际文件操作完成。系统应用主体受保护。归属不明的历史 `/data` 根文件不会自动删除；应用根目录约定不限制 Lua `io` 的绝对路径。完整契约见[应用生命周期](docs/architecture/app-lifecycle.md)。
