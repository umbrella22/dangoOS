# os：ESP32-S31 固件与系统库

面向 ESP-Mosaico v1.2 团子终端的固件与主机可测试系统库。
`src/lib.rs` 提供主机与设备共用的规范模块树，使用 `no_std + alloc`。
HAL、执行器和设备任务在实际子模块上通过 `target_os = "none"` 编译条件隔离。
`src/main.rs` 保留固件描述符与入口宏，初始化、服务接线和任务启动位于 `boot`。
完整目录见 [源码结构](../../docs/architecture/project-structure.md)。

| 库模块 | 职责 |
| --- | --- |
| `boot` | 堆 region、硬件与执行器初始化，服务启动及存储恢复接线 |
| `companion` | 行为与个性、常驻 brain、角色动画/渲染/皮肤和 Home 控件 |
| `shell` | 唯一路由、页面、组件、主题、生命周期协调和 UI 循环 |
| `apps` | 应用注册与管理、前台 Lua VM 和 native UI 桥接 |
| `services` | 音频、相机、无线、输入、显示、反馈、电源、偏好、计时器与更新 |
| `platform` | 板型、外设驱动、共享总线、存储、执行器及 C 运行支持 |
| `console` | USB CDC 日志/传输、协议会话和功能请求分派 |
| `formats` | 共享 CRC32 与清单 JSON 解码 |

## 任务调度

默认 `dual-core-ui` 使用 Core1 UI/Lua 线程；Core0 主线程运行输入、传感器和无线，
Core0 服务线程处理同步文件与资源工作。显示和音频继续使用 Core0 中断执行器。
关闭默认 feature 时复用 Core0 线程执行器。核心绑定、栈和优先级由
`boot` 与 `platform/runtime/executors` 装配，详情见 [双核任务](../../docs/architecture/dual-core.md)。

| 任务入口 | 执行位置 | 行为 |
| --- | --- | --- |
| `services/display/plane.rs` | Core0 中断，FROM_CPU_INTR1，优先级 2 | 独占 CO5300；352×336 Home 画布位于面板 (64,88)，按既有分块协议执行 QSPI DMA；Screen/Snapshot/Pool 租约保护在途像素 |
| `shell/runtime/mod.rs` | Core1 UI 线程 | 独占 LVGL；传感器与指针按既有顺序处理；50 ms 动画节奏及紧急变更绘制、导航和帧交接 |
| `apps/lua/task.rs` | Core1 UI 线程 | 执行前台应用 VM；停止状态在 VM 析构后发布，原生桥接清理继续由 UI 任务处理 |
| `boot::usb_task`、`console` | Core0 服务线程 | USB 栈与 CDC 收发、日志及延迟存储挂载；启动时优先提供控制台 |
| `services/input/touch.rs`、`sensors.rs` | Core0 主线程 | CST9220、BMI270、BMM150、电量与扩展槽输入，输出有界事件 |
| `services/feedback/haptic`、`lights` | Core0 主线程 | RMT 马达波形与扩展板 WS2812 心情灯 |

控制台是 USB-OTG HS CDC-ACM 设备。日志格式化为固定 160 字节记录，经有界通道
排队，主机打开端口后由发送任务写入 CDC。应用日志经该端口输出。

Home 顶部状态栏与角色共用画布和双槽池。无线服务按现有一秒循环读取当前 station
RSSI，界面显示 Wi-Fi 图标、信号/状态及蓝牙状态；未知信号显示 `--dbm`，断开清除旧值。
连接与断开表情在原生回退和内置 Lua brain 中均持续 4 秒，然后恢复常规行为。

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
