# ikaros 风扇控制系统

Rust + Slint 风扇控制界面，包含桌面模拟器和 ESP-Mosaico (v1.2) 真机固件。当前真机配置为 CO5300 480×480 QSPI AMOLED、CST9220 触摸及 GPIO12 的 RMT PWM。应用使用 `no_std` 和 `esp-hal`，显示/触摸协议对齐官方 esp-mosaico BSP 与组件。

## Rust workspace

| Crate | 职责 |
| --- | --- |
| [`fan-core`](crates/fan-core/) | 与平台无关的 AUTO 温度曲线和功率估算，`no_std` |
| [`fan-ui`](crates/fan-ui/) | 共享 Slint 布局、生成的组件和中文字体 |
| [`fan-desktop`](crates/fan-desktop/) | 桌面入口、Slint 事件循环和模拟设备 |
| [`fan-firmware`](crates/fan-firmware/) | S31 启动、板级配置、显示/触摸/风扇驱动和 Slint 平台 |

根目录为虚拟 workspace，统一维护依赖、`Cargo.lock`、工具链和 release 配置。默认成员是桌面程序；硬件代码在 `crates/fan-firmware/src/`。

## 常用命令

在仓库根目录执行：

```bash
cargo run                             # 480×480 桌面预览
cargo desktop                        # 480×480 方屏预览
cargo firmware                        # 构建 S31 release 固件
cargo flash                           # 构建、烧录并打开串口
cargo test-core                       # 共享控制策略测试
./scripts/check.sh                    # 桌面、固件及依赖隔离检查
./scripts/make-release.sh v17          # 生成版本包；版本名按实际发布填写
```

MCU 目标为 `riscv32imafc-unknown-none-elf`，根目录 `.cargo/config.toml` 按目标设置链接参数。桌面与固件构建无需切换配置文件。

打包需要支持 `esp32s31` 的 `espflash` 和 Python 3.11+，产物写入 `releases/`，包括 app、4MB 合并镜像、ELF、`BUILD.json` 和校验和。已有同名包会报错，防止覆盖。

## 当前能力

- 启动页显示 `ikaros` 标识渐显与短线展开，约 1 秒后切入主界面；桌面和固件共享该效果。
- 真机显示采用 PSRAM 双帧缓冲、4×10 行内部 RAM bounce buffer 和 AXI DMA 连续扫描。显示驱动显式配置 `OUT_EOF_MODE=0`；故障证据和验证范围见 [v16 说明](docs/releases/v16.md)。
- 触摸选择 0/25/50/75/100/AUTO；固件以 25kHz PWM 控制风扇。
- 真机温度、CPU 温度和转速显示仍为模拟数据，功率为估算值；实际温度传感器和 TACH 反馈尚未接入。
- 桌面与固件共用 480×480 方屏布局。Mosaico QSPI 显示与 CST9220 触摸驱动已实现（对齐官方 BSP v1.2），待真机验收。
- `esp-bootloader-esp-idf` 提供应用元数据，版本包使用已有的预编译 bootloader；应用运行时为 Rust / esp-hal，构建不需要安装 ESP-IDF。

## 文档

- [快速开始](QUICKSTART.md)
- [crate 与源码结构](docs/PROJECT_STRUCTURE.md)
- [开发、检查和发布](docs/DEVELOPMENT.md)
- [feature 与编译边界](docs/TREE_SHAKING.md)
- [Mosaico v1.2 硬件真相](docs/MOSAICO_V12_HARDWARE.md)
- [固件说明](crates/fan-firmware/README.md)
- [历史参考](docs/legacy/README.md)：未参与构建的早期驱动草稿和旧设计说明
