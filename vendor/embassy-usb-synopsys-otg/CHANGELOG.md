# embassy-usb-synopsys-otg 变更记录

记录该项目的重要变化，格式参考 [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)，版本遵循[语义化版本](https://semver.org/spec/v2.0.0.html)。以下为上游变更记录的本地中文译文。

<!-- next-header -->
## 未发布

## 0.4.0 - 2026-05-28

- **不兼容变更：** `State`/`HostState` 改为类型擦除，`OtgInstance`/`OtgHostInstance` 不再使用泛型，端点在 `State` 中分配，设备/主机驱动移除 const 泛型。静态状态需用 `StateStorage::new()`/`HostStateStorage::new()` 构造，再调用 `as_state()`/`as_host_state()` 获取状态引用。
- **不兼容变更：** `OtgState`/`OtgHostState` 改为类型擦除，`OtgInstance`/`OtgHostInstance` 不再使用泛型，端点在 `State` 中分配，设备/主机驱动移除 const 泛型。
- **不兼容变更：** 移除 `OtgInstance::endpoint_count` 和 `OtgHostInstance::channel_count`。
- **不兼容变更：** 中断处理函数移除 `endpoint_count`、`channel_count` 参数。
- 允许使用 16 个主机通道。

## 0.3.3 - 2026-05-04

- 新增 `host` 特性，支持 embassy-usb-host。
- 实现远程唤醒。

## 0.3.2 - 2026-03-10

- 禁用 OUT 端点不再刷新对应 IN 端点的 TX FIFO。
- embassy-sync 升级至 0.8.0。

## 0.3.1 - 2025-08-26

- 改进接收性能，提高从 FIFO 复制的效率。

## 0.3.0 - 2025-07-22

- `embassy-usb-driver` 升级至 v0.2.0。

## 0.2.0 - 2024-12-06

- 修复 CONTROL OUT 传输损坏，并移除 `quirk_setup_late_cnak`。
- 修复启用 `defmt` 时的构建。
- 为 H7RS 系列添加 USBPHYC 时钟配置。
- 支持 ISO 端点。
- 支持全速 ULPI 模式。
- 添加 OTG 核心 DMA 地址寄存器。
- 保证 `endpoint_count < MAX_EP_COUNT` 时端点分配失败。
- 新增配置项 `xcvrdly`（收发器延迟）。
- `EpState` 实现 `Send` 和 `Sync`。
- `vbus_detection` 默认值改为 `false`。

## 0.1.0 - 2024-04-30

首次发布。
