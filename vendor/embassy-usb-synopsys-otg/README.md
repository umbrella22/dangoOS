# Synopsys USB OTG 核心的 Embassy USB 驱动

此 crate 为 Synopsys USB OTG 设备实现 [`embassy-usb-driver`](https://crates.io/crates/embassy-usb-driver)。

它包含所有使用 Synopsys OTG IP 的芯片共用的驱动核心，不包含时钟设置、GPIO 复用等芯片专属初始化。通常通过负责初始化的 HAL 使用它。

集成此驱动的 HAL 包括：

- [`embassy-stm32`](https://crates.io/crates/embassy-stm32)：STMicroelectronics STM32 芯片。
- [`esp-hal`](https://crates.io/crates/esp-hal)：乐鑫 ESP32 芯片。

将此 crate 集成到其他设备 HAL 时，需要补充设备专属初始化，上述 HAL 可作为示例。本仓库的本地修改与验证边界见[补丁说明](PATCHES.md)。此 README 为本地中文译文，许可证和原始来源记录保留。
