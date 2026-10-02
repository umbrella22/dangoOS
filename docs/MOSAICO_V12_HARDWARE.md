# ESP-Mosaico v1.2 硬件真相（官方 BSP 提取）

> 来源：官方 BSP [esp-mosaico/esp-mosaico-bsp](https://github.com/esp-mosaico/esp-mosaico-bsp) v0.2.0（Apache-2.0，本地克隆 `~/Downloads/esp-mosaico-bsp`），出厂固件 [esp-mosaico-claw](https://github.com/esp-mosaico/esp-mosaico-claw) v0.4.0 镜像逆向交叉验证。
> Rust 侧常量已落在 `crates/fan-firmware/src/board_mosaico.rs`。
> **本机硬件：v1.2**（2026-09-30 确认）。采集日期：2026-09-30。

## 0. 为什么必须有这份文档：v1.0 → v1.2 引脚差异巨大

| 功能 | v1.0 | v1.2 | 后果 |
|---|---|---|---|
| LCD SCL | GPIO44 | **GPIO42** | 互换 |
| LCD RST | GPIO42 | **GPIO44** | 互换 |
| 主板 I2C SDA | GPIO0 | **GPIO56** | 挪位 |
| 主板 I2C SCL | GPIO1 | **GPIO3** | 挪位 |
| CODEC_PW | GPIO56（高有效） | **取消，常供电** | GPIO56 让给 I2C |
| 状态 LED | GPIO3 | **无** | GPIO3 变成 I2C SCL |
| 扩展槽 I2C | 与主板共享 I2C0 | **专用 I2C1**（SDA=0/SCL=1） | 两个 I2C 控制器 |

**我们 2026-08-31 的 Mosaico 适配按当时文档记录的全是 v1.0 引脚**——如果照旧烧录，屏不亮、触摸不通、I2C 全挂。v1.1 出厂按 v1.2 引脚接线。

### 硬件版本运行时检测（官方做法，照抄）

eFuse `USER_DATA` 块（BLOCK3）前 16 位 = `major<<8 | minor`（小端字节序，即 byte0=minor、byte1=major）：

- `(1,0)` → V1_0
- `(1,1)` / `(1,2)` → 按 V1_2
- 其他 → 官方 BSP 直接拒绝启动（`ESP_ERR_NOT_SUPPORTED`）

Rust 实现：`esp_hal::efuse::read_field_le::<u16>(esp_hal::efuse::fields::BLOCK_USR_DATA)`（需 `unstable` feature，当前锁定 rev bb239a0f 已支持 S31 eFuse）。已实现于 `board_mosaico::board_variant()`。

## 1. v1.2 完整引脚表

### CO5300 QSPI AMOLED 480×480（SPI2_HOST）

| 信号 | GPIO | 备注 |
|---|---|---|
| SCL | 42 | v1.2 新值 |
| RST | 44 | v1.2 新值 |
| CS | 50 | |
| D0-D3 | 36 / 51 / 35 / 9 | |
| TE | 43 | 可选防撕裂；官方有 TE 同步模式（`ESP_LV_ADAPTER_TEAR_AVOID_MODE_TE_SYNC`） |

- 像素时钟 **40MHz**，RGB565，窗口 gap = 0（0x2A/0x2B 全窗 0..479）。
- 官方显示 DMA 按 **8 行**一块切（`BSP_LCD_DMA_CHUNK_LINES=8`），设置 **`psram_dma_direct = false`**。该配置描述 CO5300 的 QSPI 发送路径，不能据此推断 LCD_CAM 的接收能力；当前锁定的 `esp-hal` 在 S31 上支持 AXI GDMA 接收至 PSRAM。
- **QSPI 脏区硬约束：X 坐标必须 4 像素对齐**（官方 `round_qspi_area`：x1 向下取整到 4 的倍数，x2 向上补齐）。未来 Mosaico 上做 Slint 脏区/局部刷新必须遵守。
- 官方会调 **GPIO drive capability**（SCL+D0-3 五根线，Kconfig 可配）——信号完整性旋钮，若 40MHz 下有花屏先调这个。
- 旋转全走 **MADCTL(0x36)** 的 swap_xy/mirror，安装朝向基准 = 无 swap 无 mirror（v1.2 玻璃）。

### 触摸 CST9220

| 信号 | GPIO | 备注 |
|---|---|---|
| INT | 6 | 低有效，无 RST 脚 |
| I2C | 主板 I2C0（SDA=56/SCL=3） | 400kHz |

- 官方组件 `espressif/esp_lcd_touch_cst9220 ^0.1`（我们记忆里 CST9220@0x5A 正确；"CST9217" 是 BSP 文档里的笔误混用）。
- **CST9220 的 7-bit 地址是 `0x5A`**。Claw 的 `board_devices.yaml` 用 8-bit 写地址 `0xB4`；线上读地址为 `0xB5`。`esp-hal` 接受 7-bit 地址并自行添加读写位，因此传入 `0x5A`，不能再次右移成 `0x2D`。固件统一使用 `board::TOUCH_I2C_ADDR`。
- **I2C 事务超时必须设 100ms**（官方注释：留 0 会死等，且死等发生在显示锁内）。
- 触摸坐标 mirror 约定：esp_lcd_touch 的 mirror 是 `(max - value)`，x_max/y_max = 479。

### 电源轨（上电顺序是硬约束）

| 信号 | GPIO | 电平 | 备注 |
|---|---|---|---|
| VCC_3V3（VCC_PW） | 60 | **低有效（0=开）** | 显示/触摸/传感器初始化**之前**必须先开 |
| PWR_SW（关机信号） | 57 | 开漏，拉低=断电请求 | 释放=高阻 |
| CODEC_PW | v1.0=56 | 高有效 | **v1.2 无此控制**，codec 常供电 |

- 官方上电用 **LEDC 100kHz 8-bit 占空比斜坡**（时长可配）限涌流——高侧开关栅极跟随使能脚均值，斜坡渐开；斜坡完成后把 pad 还给 GPIO（保深睡 hold 生效）。
- 深睡前：`VCC_PW`/`PWR_SW` 要 `gpio_hold_en` 保持状态。
- 断电/深睡前必须**把 LCD CS(50) 浮空**（`gpio_reset_pin` + disable 模式）：SPI 收尾后 CS 仍被驱动为高，VCC 一切它会向面板漏电。

### 按键 / 马达 / LED

| 功能 | GPIO | 备注 |
|---|---|---|
| AI 键 | 7 | 低有效 |
| BOOT 键 | 61 | |
| 马达 | 8 | 高有效；官方可选 5kHz/10bit PWM 调强度 |
| 状态 LED | （v1.0=3） | **v1.2 无 LED** |

### SPI NAND flash（SPI3_HOST，走 SD 焊盘复用）

CLK=20 / D=21 / Q=22 / CS=23 / HOLD=24 / WP=25（原理图 NAND_* → SD_*）。

## 2. CO5300 寄存器级知识（esp_lcd_co5300 ^2.2 + BSP display.c）

**QSPI 命令格式**：寄存器写使用单线 `[0x02, 0x00, reg, 0x00, params…]`；像素写使用单线头 `[0x32, 0x00, 0x2C, 0x00]`，随后像素数据走四线。esp-hal 的 24-bit Address 为 `reg << 8`，地址相位为 `Single`。分块重新发送 RAMWR 时逐块设置对应 RASET 行窗口。依据为官方 claw 的 `mosaico_boot_splash.c` 中 `lcd_command()`、`lcd_begin_pixels()` 和 `lcd_set_window()`。

**完整冷启动初始化序列**（带延时的只有首尾两条）：

```
0x11 SLPOUT                     延时 600ms
0xFE 0x20    0x19 0x10    0x1C 0xA0    0xFE 0x00    0xC4 0x80
0x3A 0x55    （RGB565）
0x35 0x00    （TE on，可选）
0x53 0x20    （BCTRL：打开亮度控制——0x51 才生效的前提）
0x51 0xFF    （亮度 0-255）
0x63 0xFF
0x2A 00 00 01 DF    0x2B 00 00 01 DF    （全窗 0..479）
0x29 DISPON                     延时 600ms
```

状态控制：

| 操作 | 序列 | 备注 |
|---|---|---|
| 亮度 | 直接写 `0x51`（0-255） | 不改变显示状态；BSP API 按百分比 ×255 映射 |
| 熄屏 | `0x28`（Display Off）→ `0x10`（Sleep In） | |
| 亮屏 | `0x11`（Sleep Out）→ `0x29`（Display On） | |
| **Deep Standby** | 先熄屏 → `0x4F`（DSTBON）→ **CS 浮空** | **退出必须 RESX 硬复位 + 全量重新初始化**；只适合深睡/关机路径 |
| 旋转 | `0x36` MADCTL swap/mirror | |

**Boot splash 交接**（官方出厂固件的招）：bootloader 已完成复位+SLPOUT+RGB565+DISPON 时，应用跳过 `0x11`/`0x51`/`0x29`（重放会把画面打黑 ~1.2s），只重放幂等的厂商配置命令。我们做自定义 bootloader 启动画面时照抄。

## 3. 传感器（全挂主板 I2C0，400kHz；共享中断 GPIO2，低有效，二极管线或）

### BMI270（IMU）@ 0x69

- 官方走 Bosch `bmi2` API（组件 `espressif2022/bmi270 ^1.1`）：`bmi270_init` 会装载 config 文件（`load_status` 可查装载结果）。
- 配置模式：accel `odr/range + bwp=NORMAL_AVG4 + filter_perf=PERF_OPT`，gyro `odr/range + bwp=NORMAL + noise_perf=POWER_OPT`。
- 物理量换算：`raw × full_scale / 32768`，accel full_scale ∈ {2,4,8,16}g，gyro ∈ {2000,1000,500,250,125}°/s。
- I2C 读写约束：单次写 ≤32 字节（BSP 适配层硬限制）。

### BMM150（磁力计 ×2，用于磁吸交互感知）@ 0x11 / 0x12

- Bosch `bmm150_sensorapi`；初始化 = NORMAL 电源模式 + REGULAR preset。
- **坑：首次转换落地前数据寄存器全 0，补偿算法会把它变成溢出样本**——官方先轮询 DRDY（寄存器 `BMM150_REG_DATA_READY_STATUS`），5ms 间隔、200ms 超时。

### BQ27220（电量计）@ 0x55

- 官方组件 `espressif/bq27220`；**初始化后要重新 seal**（ Cedv 参数写完封回去）。
- 官方默认 CEDV 曲线（两颗 80mAh 电芯 25°C 实测的过渡参数，**风扇项目如果用电池可直接抄**）：
  - `design_cap=80mAh, EDV0=3000 / EDV1=3410 / EDV2=3530 mV, EMF=3670`
  - `C0=115, R0=968, T0=4547, R1=4764, TC=11, near_full=5, self_discharge=20`
  - DOD 表：4147/4002/3969/3938/3880/3824/3794/3753/3677/3574/3490（0→100%）
  - gauging flags：CCT/SC/FCC_LIM/FC_FOR_VDQ/IGNORE_SD 全开
- 可读：电压/瞬时电流/温度(0.1K)/平均电流/平均功率/峰值负载/待机电流/剩余容量/满充容量/循环次数/续航时间/SOC/SOH。

### ES8311（音频 codec）@ 7-bit 0x19

- I2S0 主模式、标准 Philips 时序、**MCLK ×256**；MCLK=54 / BCLK=37 / WS=49 / DOUT=52 / DIN=40 / **PA=45**。
- v1.0 上电顺序：VCC_PW → CODEC_PW(56) → **等 10ms** → I2S/codec 初始化；v1.2 直接 VCC_PW → 初始化。
- esp_codec_dev 配置要点：`use_mclk=true, no_dac_ref=true, master_mode=false`，PA 增益按 `pa_voltage=5.0V / codec_dac=3.3V`。

## 4. PPA 2D 加速器查证结论（2026-09-30）

- **S31 有 PPA**：IDF v6.2-dev 含 `esp_hal_ppa/esp32s31`；BSP 的 LVGL adapter 有 `enable_ppa_accel` 开关（出厂固件默认 false）。
- **Rust 侧现状**：PAC `esp32s31 0.3.0` **已暴露 PPA 外设**（`PPA = Periph<RegisterBlock, 0x2034_5000>`，中断 102），寄存器覆盖：双层 blend（色键/固定 α/CLUT）、SRM 缩放旋转镜像、RGB2GRAY、SRAM 控制。
- esp-hal rev bb239a0f **没有**安全驱动——要用只能 PAC 直写寄存器（或等官方驱动）。
- 对风扇项目：静态 UI 用不上；若未来要做动画/特效，blend+SRM 是软光栅之外的第二条路。**旧记忆「S31 无 PPA（P4 才有）」作废**。

## 5. 组件版本基线（官方 BSP idf_component.yml）

`espressif/esp_lcd_co5300 ^2.2`（我们当时对齐的是 v2.1.0，升级时注意 diff）/ `esp_lcd_touch_cst9220 ^0.1` / `esp_lvgl_adapter 0.6.2` / `lvgl ^9.5` / `esp_codec_dev ~1.6` / `bmi270 ^1.1` / `bmm150_sensorapi ^1.0` / `bq27220 ^0.1.1` / `spi_nand_flash ^1.3`。

## 6. 移植注记（给未来 Mosaico 变体）

1. 旧双变体（mosaico feature + CO5300 QSPI 驱动 + Slint 1.17）代码在 git 历史 `ef7ba2e`（"添加 ESP32-S31 双变体支持"）～删除前可捡回，按 v1.2 引脚改 `board_mosaico.rs` 常量即可。
2. CO5300 命令头遵循 §2 的单线格式；旧实现的四线地址 `[reg,0,0]` 与官方协议不符。RGB565 像素从 CPU 小端内存转换为高字节在前后发送。
3. Mosaico 上 Slint 脏区如果启用，x 对齐 4 像素；上屏管线仍按「内部 RAM BB + out_eof 搬运」套路（官方 `psram_dma_direct=false` 印证）。
4. 40MHz QSPI + 480×480 RGB565 全帧 ≈ 11.5ms 纯传输，理论全帧率 ~87Hz——比 Korvo-1 的 35Hz RGB 宽裕得多。

## 7. 背部磁吸扩展槽（subboard）与模块生态

> 来源: BSP `subboard.c/.h` + `mosaico_module_mgr` + 三模块组件（camera/joystick/interact）源码。
> Rust 常量: `board::subboard`。

### 7.1 槽位电气架构

- 双槽各一根磁吸连接器: **6 针通用 GPIO + 6 对延长触点 + I2C + 供电**（VCC_3V3 总轨）。
- v1.2 上槽位 I2C 走**专用 I2C1**（SDA=0 / SCL=1, 内部上拉）；v1.0 与主板共享 I2C0。
- **模块身份发现**: 每个模块带 AT24C02 EEPROM（0x86 字节 V1 描述符, magic `"ESP"`），槽位地址选择脚接 EEPROM A0——左槽 GPIO14 拉低 → EEPROM@0x50，右槽 GPIO39 拉高 → EEPROM@0x51。**两槽插同一模块自动区分左右**。
- 描述符含: board_type / board_id / 硬软件版本 / vendor / capability flags / 序列号 / 32 字节板名 / 三段 CRC16 / **64 字节厂商自定义参数块**（自制模块的自由空间）。
- 右槽 180° 旋转布线，代码里用 `bsp_subboard_map_gpio()` 做左右镜像换算。

### 7.2 槽位引脚表（canonical 左槽 ↔ 右槽镜像）

| 连接器位 | 左槽 | 右槽 | 备注 |
|---|---|---|---|
| H2 | GPIO53 | GPIO46 | 摇杆 Y / 交互 LDR / 相机 RESET |
| H4 | GPIO48 | GPIO47 | 摇杆 X / 交互 IR / 相机 PWDN |
| H6 | GPIO13 | GPIO11 | 摇杆键4 / 交互 KEY_L / 相机 D7 |
| H8 | GPIO12 | GPIO10 | 摇杆键3 / 交互 KEY_R / 相机 D5（⚠️ 我们的 FAN_PWM） |
| H10 | GPIO14 | GPIO39 | **EEPROM A0 地址选择** / 相机 D4 |
| H12 | GPIO4 | GPIO5 | 摇杆键1 / 交互 PIR / 相机 D3 |
| 延长1 | GPIO16 | GPIO40 | 摇杆 B 键 / 相机 D0 / **音频 DIN** |
| 延长2 | GPIO15 | GPIO38 | 摇杆键2 / 相机 D1 / WS2812 |
| 延长3 | GPIO17 | GPIO37 | 相机 PCLK / **音频 BCLK** |
| 延长4 | GPIO18 | GPIO54 | 相机 D6 / **音频 MCLK** |
| 延长5 | GPIO19 | GPIO52 | 相机 HREF / **音频 DOUT** |
| 延长6 | GPIO55 | GPIO49 | 相机 VSYNC / **音频 LRCLK** |

**延长触点直通板上音频 I2S 总线**——插在右槽的模块可以拿到完整 I2S（音频模块的路），左槽相机则占用其中 DVP 复用脚。

### 7.3 官方三模块引脚占用（canonical 左槽值，右槽经 map_gpio 镜像）

**摇杆模块（board_type=HANDLE 0x04）**:
- X 轴 = GPIO48、Y 轴 = GPIO53（ADC oneshot，10-12bit，非阻塞校准状态机）
- 按钮 B/1/2/3/4 = GPIO16 / 4 / 15 / 12 / 13（B 在延长触点上，带标志位；其余低有效）

**交互模块（INTERACT 0x16）**:
- 光敏 LDR = GPIO53、红外发射 IR = GPIO48（RMT NEC）、左键 = GPIO13、右键 = GPIO12
- 人体感应 PIR = GPIO4、WS2812 = GPIO15；按钮支持电容触摸优先/GPIO 回退（触摸挂槽位 I2C）

**相机模块（CAMERA 0x07, 仅左槽）**:
- DVP 8-bit: D0-D7 = 16/15/33/4/14/12/18/13（GPIO33、GPIO14 也被占用）
- VSYNC=55 / HREF=19 / PCLK=17 / RESET=53 / PWDN=48 / **XCLK=NC（模块自带时钟）**
- 闪光灯 = 主机侧 **GPIO34**，开漏输出**低电平点亮**，驱动初始化即强制关闭
- EEPROM 固定 0x50（无地址线），传感器 OV3640 / SC101IOT，硬件 JPEG 解码（S31 PPA/JPEG 外设）
- EEPROM 与传感器 SCCB 都走槽位 I2C1（SDA=0/SCL=1）。OV3640 的 7-bit 地址为 `0x3C`，寄存器地址为 16 位；SC101IOT 为 `0x68`，通过 `0xF0` 选择页后使用 8 位寄存器地址。两者 PWDN 唤醒电平相反：OV3640 为低，SC101IOT 为高。
- 当前 HAL 的相机接收使用 AXI DMA，描述符必须在片内 DRAM，数据缓冲可以在 PSRAM。接收后通过 `DmaRxBuf::number_of_received_bytes()` 同步描述符和数据缓存，再读取像素；`as_slice()` 本身不执行缓存同步。`CameraTransfer::is_done()` 仅表示外设已停，完整帧还必须通过 EOF 和实际字节数检查。

### 7.4 模块生态规划（EEPROM board_type 全表）

CORE 0x01 / **POWER 0x02（电池扩展）** / **DOCK 0x03（底座）** / HANDLE 0x04 / BALANCE_CAR 0x05 / DISPLAY 0x06 / CAMERA 0x07 / SENSOR 0x08 / IO_EXP 0x09 / TOF 0x10 / MATRIX_LED 0x11 / THERMAL 0x12 / RELAY 0x13 / INTERACT 0x16。

**自制模块路线**: 任意硬件 + AT24C02 写入 "ESP" 描述符（board_type 自选或新编码, param 块 64B 自由），插上即被 module_mgr 发现（默认 250ms 扫描、3 次去抖、热插拔事件、claim/release 独占租约）。

### 7.5 「手持控制器」落地要素

1. **输入**: 摇杆模块（5 键 + 双轴）即插即用；或交互模块双键 + 板载 AI 键(GPIO7)。
2. **供电**: 主板自带 80mAh 电芯（BQ27220 计量, CEDV 曲线见 §3）只够短时; 长续航要等官方 POWER 模块或自制电池底座。
3. **低功耗**: 官方 `low_power_wakeup` 示例 + `bsp_power_prepare_sleep()`（CO5300 Deep Standby 0x4F + CS 浮空 + NAND 待机 + gpio_hold）。
4. **音频反馈**: ES8311 + PA 在主板上开箱即用; 槽位延长触点还能外接第二路 I2S。
5. **⚠️ FAN_PWM(GPIO12) 冲突**: 左槽 H8 与摇杆键3/交互 KEY_R 复用——手持模式必须先 `module_mgr` 确认左槽无模块再启用风扇 PWM，或把风扇 PWM 挪脚（DOCK 自制底座方案下引脚自由）。
6. **风扇底座（DOCK 0x03）建议**: 风扇 PWM/转速回读走底座连接器，主机拿走时底座断电——这正是官方 Dock 板类型的用途。
