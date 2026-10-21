# 快速开始

所有命令在仓库根目录执行。`rust-toolchain.toml` 选择 stable Rust，并声明 S31 使用的 RISC-V target。

## 当前状态

- 屏幕角色是狐狸团子：`crates/mosaico-s31/src/fox`（128 色调色板行 RLE 精灵 + 表情播放器，
  352×336 画布，面板原点 (64,88)），素材由 `assets/fox-dango/tools/fox_pack.py` 从
  `assets/fox-dango/device` 生成。
- 当前镜像 `target/mosaico-s31-fox2.bin`（1,598,800 B，SHA-256 `6e57a83d...`）。
  2026-10-21 实测无复位：`chunks=11`、`ras` 10.1–15.0 ms（p50 10.4，带符号部件时约 15）、
  `tx=13–14` ms、`fps=18–20`、`drop=0`。
- 原 grok-ball 矢量小球 crate 已移除，行为逻辑（affect、personality）并入
  `crates/mosaico-s31/src/behavior/`，其余在 git 历史里（`710d9dc` 及之前）。
- 详细顺序见 [docs/plans/ROADMAP.md](docs/plans/ROADMAP.md)（历史文档，含 grok-ball 阶段）。

## 常用命令

```bash
# 库的 host 测试（behavior / fox / ui / telemetry）
cargo test -p mosaico-s31 --offline

# 纯驱动模块（触摸报告解码、窗口打包、面板几何）
./scripts/test-drivers.sh

# 16 表情 LVGL host 预览（480x480）
cargo run -p mosaico-s31 --example preview --offline

# 启动页 / 入场动画（PPM）
cargo run -p mosaico-s31 --example splash_preview --offline

# 固件目标构建
cargo fw --offline

# 以上全部
./scripts/check.sh

# LVGL UI 层测试
cargo test -p lvgl-ui --manifest-path ../lv_binding_rust/Cargo.toml -- --test-threads=1
```

如果 registry 很慢，在命令前加 `CARGO_NET_OFFLINE=true`。依赖缓存和 `Cargo.lock` 已完整。

## 烧录

只写 `main_app`（`0x210000`），不要动 bootloader、分区表或 `0x20000` 的 `vibe_mode`。

```bash
# 板子进入下载模式后
./scripts/flash-app.sh /dev/cu.usbmodemXXXX
```

控制台是 **USB-OTG HS CDC-ACM**，不是 USB-Serial-JTAG，应用运行时枚举为
`/dev/cu.usbmodemmosaico_s31_r11`。统计行的含义见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

## 目录

- `crates/mosaico-s31`：唯一的 crate，库（host 可测）+ 固件，结构见 [docs/PROJECT_STRUCTURE.md](docs/PROJECT_STRUCTURE.md)
- `assets/fox-dango`：角色素材与生成脚本
- `../lv_binding_rust/lvgl-ui`：`App`/`Node`/`Ui` 声明式 UI 层
- 渲染管线讲解：[docs/FOX_RENDER_PIPELINE.md](docs/FOX_RENDER_PIPELINE.md)

引脚、电源轨和扩展槽定义见 [Mosaico v1.2 硬件真相](docs/MOSAICO_V12_HARDWARE.md)。
