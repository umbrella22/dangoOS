# 快速开始

所有命令在仓库根目录执行。`rust-toolchain.toml` 选择 stable Rust，并声明 S31 使用的 RISC-V target。

## 桌面预览

```bash
cargo desktop
```

打开 480×480 方屏预览（Mosaico 同尺寸）。桌面数据由模拟设备生成，按秒更新。macOS、Windows 和 Linux 使用 Slint 桌面后端；Linux 需要对应后端的窗口系统开发库。

## 编译固件

```bash
rustup target add riscv32imafc-unknown-none-elf
cargo firmware --locked
```

ELF 输出到 `target/riscv32imafc-unknown-none-elf/release/fan-control-fw`。固件启动时自检 eFuse 硬件版本（仅接受 v1.1/v1.2 引脚映射）。真机运行使用 release 构建。

## 烧录和串口

安装支持 `esp32s31` 的 `espflash` 后，连接板载 Type-C UART：

```bash
cargo flash --locked
```

该命令会写入设备并打开串口。串口为 115200 8N1。仅编译使用 `cargo firmware`。

## 生成交付包

```bash
./scripts/make-release.sh v17
```

`v17` 是版本名示例。脚本执行锁定依赖的 release 构建，输出 `releases/fan-control-<版本>-<日期>/` 及同名 ZIP。镜像烧录地址和校验步骤位于包内 README。

## 检查代码

```bash
cargo test-core
./scripts/check.sh
```

检查脚本分别构建桌面和 MCU 目标。该 workspace 混合 `std` 桌面程序与 `no_std` 固件，构建时按 crate 和 target 选择，不使用单一 target 的 `cargo build --workspace`。

详细入口见 [开发指南](docs/DEVELOPMENT.md)；引脚与扩展槽见 [Mosaico v1.2 硬件真相](docs/MOSAICO_V12_HARDWARE.md)。
