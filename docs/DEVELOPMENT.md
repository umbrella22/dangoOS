# 开发、检查和发布

## 开发入口

根 `Cargo.toml` 是虚拟 workspace，默认成员为 `fan-desktop`。`Cargo.lock` 同时记录桌面和固件依赖。直接寄存器访问使用 crates.io 正式版本 `esp32s31 0.3.0`。HAL 与应用元数据辅助库仍固定到同一个已验证的 Git revision；变更依赖时同时检查固件构建和真机运行。

`esp-hal 1.2.2` 虽支持 S31，但尚未开放该芯片的 `lcd_cam`，无法构建当前 RGB 显示驱动。单独将 `esp-bootloader-esp-idf` 切换为正式版 `0.6.0` 会与 Git HAL 的 `esp-rom-sys` 产生原生库 `links` 冲突，因此二者暂时保留 Git 来源。HAL 仍传递依赖 Git 版 PAC；直接依赖切换为 `esp32s31 0.3.0` 并不消除这条依赖链。

Slint 与 `slint-build` 固定为 `1.17.1`，并通过 `[patch.crates-io]` 使用同版本的软件渲染器本地副本。`1.18.0` 和 `1.18.1` 的路径采样调用了 `no_std` 下不可用的 `f32::rem_euclid`，因此暂不升级。升级 Slint 时需要同步迁移脏区裁剪、路径工作区复用及性能计数补丁，运行渲染回归测试并构建 MCU 固件；补丁说明见 [`vendor/i-slint-renderer-software/PATCHES.md`](../vendor/i-slint-renderer-software/PATCHES.md)。

```bash
cargo desktop                          # 方屏预览（480×480）
cargo check -p fan-desktop
cargo firmware --locked                # 主固件 release 构建
```

Slint 布局和字体在 `crates/fan-ui/`，两种运行环境依赖同一组件 crate。控制曲线在 `crates/fan-core/src/fan.rs`。板级和寄存器操作在 `crates/fan-firmware/src/`。桌面模拟设备只服务于预览。

桌面 `app.rs` 通过 Slint Timer 更新状态及 UI，回调在事件循环线程执行。固件 `platform_mosaic.rs` 轮询触摸，按单调时间更新游戏与 IMU，按需局部重绘单缓冲，同步分块刷新 GRAM；发送失败后重试未发布帧。GPIO12 风扇 PWM 默认关闭，仅在左槽为空且引脚独占时通过 `fan-pwm-gpio12` feature 启用。

## 检查

```bash
./scripts/check.sh
```

脚本执行格式检查、`fan-core` 契约测试、启动动画时序及 RGB565 渲染回归测试、桌面模拟器的温度与调速回归测试、桌面布局的构建检查和主固件的 release 构建，并检查 MCU 的运行时依赖不包含桌面窗口后端或 IDF 运行时库。命令失败立即退出，不清理已有 target 产物。

`./scripts/test-touch.sh` 使用 `rustc --test` 测试固件生产路径的 CST9220 地址编码与报告解析，主机无需链接 ESP32 HAL。`./scripts/test-mosaic-runtime.sh` 验证定时调度、覆盖层触点隔离、失败重试、游戏重开以及 BMI270 初始化和读数转换。模拟协议样例不代表真机抓包。

桌面软件渲染使用 `cargo test --locked -p fan-ui --features software-rendering --test software_rendering`；Mosaic 软件渲染与指针交互使用 `cargo test --locked -p fan-ui --features embedded --test mosaic_rendering`。两条命令分别编译对应 UI 根，避免把旧界面编入固件。

`./verify-tree-shaking.sh` 保留为该检查脚本的兼容入口。构建通过证明模块、feature 和链接配置可用；显示连续扫描、触摸和 PWM 的实际时序仍由真机验证。

## 固件构建边界

- MCU target 是 `riscv32imafc-unknown-none-elf`。根 `.cargo/config.toml` 仅为此 target 设置 `-Tlinkall.x` 和烧录 runner。
- `cargo firmware` 固定选择 `fan-firmware` 的 `fan-control-fw` binary 和 release 配置。
- `cargo flash` 会烧录设备；`cargo firmware` 和检查脚本仅构建。
- `cargo build --workspace` 无法用一个 target 同时满足桌面与固件依赖，检查流程分别选择 package 和 target。
- Cargo feature 会叠加。目标板唯一（ESP-Mosaico v1.2），固件固定启用 `fan-ui/embedded`。
- QSPI 线序、厂商初始化序列、GRAM 刷新路径与 eFuse 版本自检集中在 `drivers/display.rs`/`board.rs`，相关修改需对照官方 BSP/组件源码与 [Mosaico v1.2 硬件文档](MOSAICO_V12_HARDWARE.md) 验证。

## 发布

```bash
./scripts/make-release.sh v17
```

脚本从仓库根目录构建 `fan-firmware`，使用 `--locked`，将 `docs/releases/<版本>.md`（如存在）附到包内 README。产物位于 `releases/fan-control-<版本>-<日期>/` 和同名 ZIP，已有包不会被覆盖。

打包使用 Python 3.11+ 和支持 S31 的 `espflash`。默认先构建再打包；`--no-build` 复用现有 ELF，并在 `BUILD.json` 标记 `reused_elf=true`，此时当前源码哈希不能证明该 ELF 由当前源码生成。

包内包含：

- app（烧录地址 `0x20000`）和合并镜像（`0x0`）。
- 已验证的 bootloader（`0x2000`）、分区表（`0x9000`）和 ELF。
- `BUILD.json`：构建命令、工具版本、关键依赖、ELF 和 workspace 源文件哈希。
- `SHA256SUMS` 及烧录、日志判读说明。

镜像生成时校验 SHA256/MD5 和分区边界。打包不执行烧录，也不标记 `hardware_verified=true`。历史发布记录位于 `docs/releases/`，旧代码草稿位于 `docs/legacy/`。
