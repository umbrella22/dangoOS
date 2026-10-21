# 快速开始

所有命令在仓库根目录执行。`rust-toolchain.toml` 选择 stable Rust，并声明 S31 使用的 RISC-V target。

## 当前状态

G0/H0/G1/R0/R1/G2 与 H1 已在真机验收；H2（传感器→表情→马达）和 H3（人味交互、启动页、电量）已完成 host 侧并通过目标构建，**待真机手感验收**。

- `grok-ball`：110 个 host 测试。
- `lvgl-ui`：51 个测试（含 10 个自定义控件测试）。
- `mosaico-s31`：球体首屏 + 启动页/出场动画 + 交互运行时 + R1 异步显示/输入 + 传感器与电量任务 + USB-OTG HS CDC；51 个 host 测试。
- R0 真机证据：CDC、心跳、整帧 480×480、黑底+三色带、触摸。
- R1 真机证据：100 次触摸 P50 29–33ms、P95 36–41ms、76 秒无复位。
- G2 真机证据：球体 240×240，6 状态点按切换；100 次标签触摸 P50 28–29ms、P95 30ms、20–21fps；全屏透明触摸层，只有球体 240×240/8 块被重绘。
- H1.2：两颗 BMM150 在主板 I2C0 `0x11/0x12`，chip ID `0x32`，20Hz；模块 `interact(0x16)`；IMU 35–46Hz。
- 当前镜像：`target/mosaico-s31-h3-fixed4.bin`（945,920 B，SHA-256 `367b148b...`，固定球平面、无拖拽）；G2/R0/R1 验证镜像仍在 `target/`。
- `h3-fixed4` 之后的提交（表情优先级、canvas 双缓冲、ball_plane 中断执行器、渐变有序抖动）已上真机：镜像 `target/mosaico-s31-df1b695.bin`（922,896 B，SHA-256 `0a8427e9...`）。
  2026-10-21 实测约 3 分钟无复位：`ball_plane on interrupt executor (FROM_CPU_INTR1, priority 2)`、`2 canvas slots`；
  `ras` 18.9–20.0 ms（p50 19.4，抖动前约 18 ms）、`tx=6` ms、`fps=18–19`、`drop=0`、`q=0/32`。触摸手感和表情切换待人工验收。
- 缺失美术素材清单见 [docs/ASSET_REQUESTS.md](docs/ASSET_REQUESTS.md)。
- 详细顺序见 [docs/plans/ROADMAP.md](docs/plans/ROADMAP.md)。

## 常用命令

```bash
# 核心 host 测试
cargo test -p grok-ball --offline

# 六态 320x320 PPM
cargo run -p grok-ball --example states --offline

# 16 表情 BallWidget host 预览（480x480）
cargo run -p mosaico-s31 --example preview --offline

# 启动页 / 出场下落 / 猫耳狐狸耳皮肤（PPM）
cargo run -p mosaico-s31 --example splash_preview --offline

# WebP/GIF 素材 -> RGB565 Rust 表
python3 crates/grok-ball/tools/sprite_pack.py --input art/x.webp --name x --size 240

# 固件目标构建
cargo build -p mosaico-s31 --release --target riscv32imafc-unknown-none-elf

# LVGL UI 层测试
cargo test -p lvgl-ui --manifest-path ../lv_binding_rust/Cargo.toml -- --test-threads=1
```

如果 registry 很慢，在命令前加 `CARGO_NET_OFFLINE=true`。依赖缓存和 `Cargo.lock` 已完整。

## 烧录 R0

R0 镜像只写 `main_app`（`0x210000`），不要动 bootloader、分区表或 `0x20000` 的 `vibe_mode`。

```bash
espflash save-image --chip esp32s31 \
  target/riscv32imafc-unknown-none-elf/release/mosaico-s31-fw \
  target/mosaico-s31-r0.bin

esptool --chip esp32s31 --port <PORT> --baud 115200 \
  --before no-reset --after hard-reset \
  write_flash 0x210000 target/mosaico-s31-r0.bin

espflash monitor --chip esp32s31
```

R0 控制台是 **USB-OTG HS CDC-ACM**，不是 USB-Serial-JTAG。验证镜像会枚举为 `/dev/cu.usbmodemmosaico_s31_r01`，并每 5 秒重放：

```text
[boot] Mosaico S31 R0 cdc=true drops=0 q=0/32 disp=true chunks=30 ms=38 hb=10
```

`disp=true` 表示整帧已传输，`hb` 是当前 `ui_task` 心跳；触摸会打印 `touch down at (x, y)` / `touch release`。读取命令：

```bash
espflash monitor --chip esp32s31 --port /dev/cu.usbmodemmosaico_s31_r01
```

## 目录

- `crates/grok-ball`：数据、球体引擎、RGB565 光栅、表情引擎
- `crates/mosaico-s31`：S31 固件（R0）和 BallWidget host 预览
- `../lv_binding_rust/lvgl-ui`：`App`/`Node`/`Ui` 声明式 UI 层
- `archive/crates-pre-s31-20261019`：旧 Slint/风扇 crates 的本地归档，不参与 workspace 构建

引脚、电源轨和扩展槽定义见 [Mosaico v1.2 硬件真相](docs/MOSAICO_V12_HARDWARE.md)。
