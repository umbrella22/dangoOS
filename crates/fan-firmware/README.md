# fan-firmware

ESP-Mosaico v1.1/v1.2 的 `no_std` 固件，运行 480×480 Slint Mosaic Hub、CO5300 QSPI AMOLED、CST9220 触摸和 BMI270 水平仪。板级映射与上电约束见 [硬件说明](../../docs/MOSAICO_V12_HARDWARE.md)，功能范围见 [路线图](../../docs/MOSAICO_UI_ROADMAP.md)。

## 构建

在仓库根目录执行：

```bash
cargo firmware --locked
./scripts/check.sh
```

ELF 位于 `target/riscv32imafc-unknown-none-elf/release/fan-control-fw`。HAL 与 bootloader 辅助库固定到同一 Git revision，构建无需安装 IDF。固件只编译 `fan-ui` 的 Mosaic 根；桌面旧风扇界面通过另一组 feature 单独构建。

`cargo flash` 会操作设备。Mosaico 官方布局的应用地址为 `0x20000`、分区表为 `0x9000`、bootloader 为 `0x2000`；交付包由 `scripts/make-release.sh <版本>` 生成，日常更新使用 app 镜像。主机检查和打包均不代表已经完成板端验收。

## 输入、状态和显示

`src/app.rs` 从 eFuse 校验硬件版本，先开启 VCC_PW 电源轨，再初始化显示、共享 I2C 总线、触摸和 IMU，最后进入 `src/platform_mosaic.rs`。GPIO7 的 AI 键切换锁屏。

Slint 根组件处理 Hub/应用导航、抽屉和锁屏。平台把 CST9220 报告转换为指针事件，按单调时间调度 30Hz 游戏步进和 100ms IMU 采样；游戏在离开页面或显示覆盖层时暂停。天气、联网、电量和日期仍按路线图保留占位数据，数字时钟由 uptime 推算。

触摸寄存器地址按 `[高字节, 低字节, 参数…]` 发送，15 字节报告读自 `0xD000`。HYN212 协议读后 ACK 为 `[0xD0, 0x00, 0xAB]`，legacy `0x542F` 免 ACK。I2C 错误和无效报告保留上次有效指针状态。

Slint 以 `ReusedBuffer` 渲染 PSRAM 中的 RGB565 单缓冲。`Display::present()` 将整帧分成 15 个 32 行窗口，每块设置 RASET 后发送 RAMWR；内部 RAM 暂存完成 LE→BE 字节换序，SPI DMA 以 40MHz 发送。CO5300 头部 `[opcode, 0x00, reg, 0x00]` 全部走单线，寄存器参数走单线，像素数据走四线。发送失败时保留待发布帧，下一轮重试，即使 Slint 没有新脏区也会继续发布。

亮度在启动及数值变化时写入 `0x51`，成功后缓存已应用值，失败后重试。初始化序列通过 `0x53=0x20` 开启 BCTRL。Deep Standby `0x4F` 退出需要硬复位和完整初始化；断电/深睡前 LCD CS 须浮空。

## GPIO12 风扇配置

GPIO12 同时连接左槽 H8，可用作相机 D5、交互 KEY_R 或摇杆键 3。默认固件不初始化该引脚的风扇 PWM，也没有模块仲裁器。仅在左槽为空且 GPIO12 已独占接线到风扇时，使用显式构建选项：

```bash
cargo firmware --locked --features fan-pwm-gpio12
```

此配置初始化 RMT 风扇通道，目标占空比仍从 0 开始；平台保留 `set_target()` 接口，Mosaic 当前只有数据卡，尚无调速入口或模块仲裁。TACH 和温度传感器未接入，卡片不提供实测转速或温度。

## 验证与诊断

```bash
./scripts/test-touch.sh
./scripts/test-mosaic-runtime.sh
cargo test --locked -p fan-ui --features embedded --test mosaic_rendering
cargo firmware --locked --features lcd-color-test
```

前三项在主机检查生产协议、运行时状态及实际 Mosaic 组件的输入与软件渲染。`lcd-color-test` 固件先显示 15 秒 RGB565 彩条/棋盘，再进入正常界面，可用于区分显示链路和字体/透明度问题。

串口使用 115200 8N1。平台每 5 秒输出渲染次数、平均/最长渲染耗时、DMA 错误及触摸轮询次数。板端验收需检查全屏分块位置、触摸与锁屏、亮度、倾斜方向和 GPIO12 电气状态；结果记录在 [审查与验收](../../docs/MOSAICO_UI_REVIEW.md)。
