# os：ESP32-S31 固件与系统库

面向 ESP-Mosaico v1.2 团子终端的固件与主机可测试系统库。
`src/lib.rs` 提供 `os` 库，使用 `no_std + alloc`，承载不直接访问硬件的逻辑，
可以在主机运行 `cargo test`。`src/main.rs` 是 `os-fw` 固件入口，编排驱动和
Embassy 任务。完整目录见 [源码结构](../../docs/architecture/project-structure.md)。

| 库模块 | 职责 |
| --- | --- |
| `behavior` | `Expression`、情绪引擎（传感器到情绪）、个性引擎（点按）、`Runtime`（合并决策与马达请求） |
| `fox` | 调色板 RLE 精灵、`FoxAnimator`（表情和时间到 `Scene`）、`FoxRenderer`（`Scene` 到 RGB565）、入场动画 |
| `hub` | 主页、启动器与应用导航，边缘手势、页面滑动、计时器、传感器快照、Lua 应用注册与 USB 推送 |
| `ui` | `MosaicoApp` 屏幕树、`FoxWidget` / `FoxControl`、双槽 `CanvasPool`、启动器页面、中文字体与暂未启用的启动页 |
| `telemetry` | 触摸到首像素延迟和显示统计 |

## 任务调度

`esp-rtos` 与 Embassy 在 core 0 上使用线程、中断两个执行器：

| 任务 | 执行器 | 行为 |
| --- | --- | --- |
| `plane_task`（`tasks/plane.rs`） | 中断，FROM_CPU_INTR1，优先级 2 | 独占 CO5300；取得最新画布槽，将 352×336 画布上传到面板坐标 (64, 88)，按 32 行分块执行异步 QSPI DMA；动画帧等待 TE，携带触摸标记的帧跳过等待。 |
| `ui_task`（`tasks/ui.rs`） | 线程 | 独占 LVGL；每 5 ms 将传感器与触摸事件送入 `behavior::Runtime` 和 Hub，再绘制当前页面。狐狸按 50 ms 节奏或在触摸后立即绘入空闲画布槽；LVGL / Lua 页面绘入 PSRAM 影子帧，交给 plane 上传并记录统计。 |
| `lua`（`tasks/lua.rs`） | 线程 | 在内存上限和时间片约束下运行一个 Lua 应用；显示帧交给 UI 任务。 |
| `usb_task`（`main.rs`） | 线程 | USB 协议栈、日志发送、推送协议接收和延迟存储挂载；优先启动，以便读取开机崩溃记录。 |
| `input_task`（`tasks/input.rs`） | 线程 | 独占 CST9220；使用 GPIO6 边沿触发与 5 ms 兜底，通过异步 I2C 读取并写入有界通道。 |
| `sensor_task`（`tasks/sensors.rs`） | 线程 | BMI270、两个 BMM150、BQ27220、左右扩展槽（任意侧的交互板及相机识别），输出有界 `SensorEvent` 通道。 |
| `haptic_task`（`tasks/haptic.rs`） | 线程 | 通过 RMT 在 GPIO8 生成 5 kHz PWM 马达波形。 |
| `lights`（`tasks/lights.rs`） | 线程 | 控制右槽交互板的 WS2812 心情灯。 |

控制台（`console.rs`）是 USB-OTG HS CDC-ACM 设备。`log` 消息格式化为固定
160 字节记录，经有界 `embassy_sync` 通道排队，主机打开端口后由发送任务写入 CDC。
应用日志不走 `esp-println` 的 `jtag-serial` 通道，该通道没有连接这块板的 USB-C。

## 硬件约束

- CO5300 QSPI AMOLED，480×480，RGB565，SPI2 + AXI DMA；当前固件 QSPI 为
  80 MHz，官方 BSP 的 40 MHz 配置及历史对照见 [竖线排查](../../docs/troubleshooting/photo-vertical-lines.md)。
- QSPI 寄存器操作码为 `0x02`，GRAM 写入操作码为 `0x32`；`CASET` / `RASET`
  窗口包含两端，横坐标必须按 4 像素对齐。
- CST9220 在 I2C0 上使用 7 位地址 `0x5A`，SDA=56、SCL=3、400 kHz；INT 为 GPIO6。
- VCC_3V3（GPIO60）低有效，必须在初始化屏幕和触摸之前开启。
- 当前设备采用 A/B 布局，常规 OTA 更新非运行槽。手动 ROM 恢复与分区迁移边界见
  [开发指南](../../docs/guides/development.md) 和 [分区迁移](../../docs/ota/partition-migration.md)。

## 依赖

- `esp-hal`、`esp-alloc`、`esp-bootloader-esp-idf`、`esp-rtos`、`esp-storage`
  来自同一个固定 esp-hal Git 提交，使用 `esp32s31`、`embassy`、`log-04` 等 feature。
  registry 版 esp-rtos 会引入第二个 `esp-rom-sys` 而被 Cargo 拒绝，因此统一使用 Git
  来源并一起升级。
- Embassy 依赖包含 `embassy-executor 0.10`、`embassy-time 0.5`、`embassy-usb 0.6`、
  `embassy-futures 0.1`、`embassy-sync 0.8`。
- ESP 依赖位于 `[target.'cfg(target_os = "none")'.dependencies]`。
  `rs_lvgl`、`rs_lvgl_ui`、`log`、`lua-engine` 和 `fs` 是普通依赖，因此库可以在主机构建。

## 构建、测试与更新

从仓库根目录执行：

```sh
cargo test -p os --offline
./scripts/check/drivers.sh
cargo fw --offline
python3 scripts/device/console.py ota <固件.bin>
```

OTA 输入必须是已经生成并验证的固件 BIN。下载模式恢复入口为
`scripts/device/flash.sh --rom-recovery-slot0 <PORT>`；该入口只写 slot 0，并保留
已有启动选择。常规升级流程见 [USB OTA](../../docs/ota/usb-update.md)。

## 控制台

设备 USB-C 连接 USB-OTG HS。固件在 `USB_HS` 上枚举 CDC-ACM，并安装唯一的
`log::Log` 实现，所有任务中的 `log::info!` / `log::warn!` 都发送到该端口。

```sh
# 只读监视，自动查找 USB CDC 端口。
espflash monitor --chip esp32s31

# macOS 上的运行时端口。
espflash monitor --chip esp32s31 --port /dev/cu.usbmodemmosaico_s31_r11
```

如果 `espflash monitor` 没有输出，可用 pyserial 等串口工具读取。
运行时端口只在固件运行期间存在，下载模式使用另一个端口名。
