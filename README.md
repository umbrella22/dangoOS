# my-slint-fan

在 **ESP32-S31** 上用 **esp-rtos**(异步运行时)+ **Slint**(软件渲染 UI)做的风扇控制固件(Rust,no_std)。

> 目标硬件:**乐鑫 ESP-Mosaico**(可扩展智能交互套件)。2026-08-31 完成 S31 全量最新栈 + Mosaico 板级适配(CO5300 QSPI AMOLED 驱动),零警告编译通过(flash ≈ 1.0MB / 16MB),尚未烧录硬件。
>
> 历史沿革:项目起始于立创·实战派 ESP32-C3 → 迁移到 ESP32-S31-Korvo-1 → 最终定板 ESP-Mosaico。C3 平台的版本考古与踩坑记录保留在下方历史章节。

## 依赖栈(全量最新,2026-08-31 编译通过)

| crate | 版本 | 说明 |
|---|---|---|
| esp-hal | `1.2.0-rc.0` | features: `esp32s31` + `unstable` |
| esp-rtos | `0.4.0` | features: `embassy` + `esp32s31`(esp-hal-embassy 的后继)|
| embassy-executor | `0.10` | 0.10 起 task 构造器返回 Result |
| embassy-time | `0.5` | |
| embassy-sync | `0.8` | |
| esp-alloc | `0.11` | features: `esp32s31`;PSRAM 帧缓冲经 `alloc_caps(External)` |
| esp-println | `0.18` | 关 default + `["colors","uart","esp32s31","log-04"]` |
| esp-backtrace | `0.20` | features: `["esp32s31","panic-handler","println"]` |
| esp-bootloader-esp-idf | `0.6.0` | features: `esp32s31`;espflash 4.x 校验应用描述符 |
| **slint / slint-build** | **`1.17` 正式版** | features: `compat-1-2` + `unsafe-single-threaded` + `libm` + `renderer-software` |
| 目标 | `riscv32imafc-unknown-none-elf` | S31 = RV32IMAFC(带 FPU/原子指令),标准 Rust 工具链 |

之前在 C3 上「Slint 只能锁 git rev / executor 钉死 0.7」的限制全部解除 —— 根因是 C3 无硬件原子指令,
esp-hal 被迫用 `unsafe-assume-single-core`,与 Slint 的 `critical-section` 互斥(详见下方历史章节)。

## 目录结构

```
esp-32/
├── Cargo.toml            # 全量最新依赖矩阵(见上表)
├── rust-toolchain.toml   # stable 工具链 + riscv32imafc 目标
├── build.rs              # slint-build 编译 UI(EmbedForSoftwareRenderer)
├── .cargo/config.toml    # target、espflash runner(esp32s31)、linkall.x、日志级别
├── git-proxy.config      # GitHub 镜像重写配置(历史遗留,现全走 crates.io)
├── src/
│   ├── main.rs           # esp_rtos::main → 堆/PSRAM → 芯片初始化 → esp_rtos::start → QSPI → CO5300
│   ├── board.rs          # ESP-Mosaico 引脚映射表(来自官方用户指南)
│   ├── display.rs        # CO5300 QSPI AMOLED 驱动:官方初始化序列 + PSRAM 帧缓冲 + 整帧写 GRAM
│   ├── gui.rs            # Slint Platform 集成 + 渲染循环 + 灯开关回调
│   └── fan.rs            # 风扇控制任务(PWM 引脚候选 GPIO12)
└── ui/
    └── appwindow.slint   # 480×480 方形界面:风扇滑杆 + 状态灯开关(手写控件)
```

## ESP-Mosaico 板载资源(与本项目相关)

来源:[官方用户指南](https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s31/esp-mosaico/user_guide.html)

- ESP32-S31NRV16:**双核 RISC-V 最高 320MHz**,Wi-Fi 6 / BLE 5.4 / 802.15.4;封装内 16MB PSRAM,板载 16MB NOR + 1Gbit NAND
- **2.16 寸 480×480 QSPI AMOLED**:CO5300 驱动,触摸 CST9220(I2C 0x5A,INT=GPIO6)
- 橙色状态 LED @ **GPIO3**(低电平点亮)—— UI 的灯开关对应硬件
- AI 按键 @ GPIO7(低有效);振动马达 @ GPIO8(高电平开)
- 音频:ES8311(I2C @0x19,I2S BCK=37/DOUT=40/WS=49/DIN=52/MCLK=54)+ NS4150B 功放(PA=45)
- 传感器:BMI270 六轴 @0x69、双 BMM150 @0x11/0x12;电量计 BQ27220 @0x55
- 共享 I2C:SDA=GPIO0 / SCL=GPIO1
- 电源:USB-C 5V / 锂电池(TP4057 充电、SAM8108 开机控制 @GPIO57),3.3V 轨控制 @GPIO60
- 左右 2×10P 模块扩展座:空闲 GPIO 充足(**风扇 PWM 候选 GPIO12**)
- UART0 @ GPIO58/59;烧录:按住 Boot + 点 Reset 进下载模式

## 环境准备

```bash
rustup target add riscv32imafc-unknown-none-elf
brew install espflash   # 或 cargo install espflash
```

## 编译与烧录

```bash
cargo build --release
espflash flash target/riscv32imafc-unknown-none-elf/release/my-slint-fan --chip esp32s31 --monitor
# 或 cargo run(runner 已配置)
```

## CO5300 驱动实现要点(对齐 esp_lcd_co5300 v2.1.0 官方组件)

- 传输:半双工 QSPI。参数写 = 单线 opcode `0x02` → 四线 `[寄存器, 0x00, 0x00]` → 四线参数;
  帧写 = 单线 `0x12` → 四线 `[0x2C, 0, 0]` → 四线像素(RGB565 **大端**)
- esp_hal 表达:`half_duplex_write(Quad, Command::_8Bit(opcode, Single), Address::_24Bit(reg<<16, Quad), 0, data)`,
  大缓冲由驱动自动分块且 CS 保持
- 初始化:复位脉冲(GPIO42,低有效)→ MADCTL/COLMOD → 官方 `vendor_specific_init_default`
  序列(FE/C4/35/53/51/63/默认窗口/SLPOUT/DISPON,含延时)
- AMOLED 有 GRAM 自刷新:每帧写一次 GRAM 即可,无需持续扫描;TE(GPIO43)备用防撕裂
- 帧缓冲:450KB 经 `HEAP.alloc_caps(External)` 分配到 PSRAM(DMA 兼容,64 字节对齐)
- 亮度:0x51 寄存器(当前 0xFF 最大,可在 UI 加亮度条)

## Slint 1.17 API 注意(相对 1.9 的变化)

- 软件渲染整帧:`renderer.render(frame_buffer, WIDTH)`(1.9 的 `render_by_line` 仍可用于逐行流式屏)
- build.rs:`EmbedResourcesKind::EmbedForSoftwareRenderer`(替换 `EmbedFiles`)
- TouchArea 指针坐标叫 `mouse-x`;组件内回写自身属性需 `in-out property`;组件先定义后使用
- no_std 下需要 `extern crate alloc;` + `use alloc::{boxed::Box, rc::Rc};`

## esp-hal / esp-rtos 1.2.0-rc.0 + 0.4.0 注意

- 入口宏:`#[esp_rtos::main]`;调度器在 async main 内 `esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0)` 启动
- embassy-executor 0.10:task 构造器返回 Result,`spawn(task().unwrap())`;`task-arena-size-*` 特性已移除
- SPI:`Spi::new` 返回 Result;`with_dma(DMA_AXI_CH0)` 后切片 `write()` 自带分块(QSPI 半双工无需 with_dma)
- `Spi::new` + `with_sio0..with_sio3` + `half_duplex_write` 均在 `unstable` 特性后(已开启)

## 内存预算(riscv32imafc release 实测)

- flash ≈ 1.0MB(16MB 的 ~6.2%)
- 内部 RAM 静态 ≈ 74KB(堆 72KB + 少量静态);**帧缓冲 450KB 独立放 PSRAM**(16MB)
- 注意 `lto = "off"` 为规避 Slint 软件渲染器与 fat LTO 的兼容问题

## 上板调试顺序

1. 串口(USB-C)看启动日志
2. 屏幕点亮三调点:`display.rs` 的 MADCTL 方向 / 反显(INVON,当前未开)/ 初始化序列时序
3. 触摸(CST9220,GPIO6 中断)接入后经 `window.try_dispatch_event(WindowEvent::Pointer...)` 注入
4. 亮度条:UI 加滑杆写 0x51 寄存器即可

## TODO

- [ ] Mosaico 到手后首次烧录验证
- [ ] 触摸(CST9220,I2C 0x5A/INT GPIO6)接入 UI
- [ ] 风扇 PWM:LEDC + GPIO12(模块座 H2)
- [ ] 亮度调节滑杆(CO5300 0x51 寄存器)
- [ ] AI 按键(GPIO7)/振动马达(GPIO8)接入 UI 交互
- [ ] 电池电量(BQ27220 @0x55)显示

## 历史章节:实战派 C3 / Korvo-1 时期的版本考古(保留备查)

以下结论针对 ESP32-C3/C2/S2(无硬件原子指令)平台,换回这类芯片时必须重读:

1. **GitHub 直连不通** → `CARGO_NET_GIT_FETCH_WITH_CLI=true` + `GIT_CONFIG_GLOBAL=git-proxy.config`(仓库内)走 gh-proxy 镜像。
2. **embassy-executor 0.8+ 没有 `task-arena-size-*`**;C3 波次只能用 0.7(被 esp-hal-embassy 0.9.1 钉死 ^0.7)。
3. **esp-hal-embassy 0.9.1 只配 esp-hal `=1.0.0-rc.0`**(经 requires-unstable 需要 `__esp_hal_embassy` 特性,rc.1 起
   删除;esp-hal-embassy 已停更,后继 esp-rtos 在 C3 上因 `rt → critical-section` 与 unsafe-assume 互斥而编译不过)。
4. **Slint 全系正式版(1.2~1.17)在 C3 上不可用**:i-slint-core 硬编码 `portable-atomic/critical-section`
   与 C3 的 `unsafe-assume-single-core` 互斥 —— 当初 DeepSeek 指南锁定的 git rev 正是摘掉该依赖的修复版。
5. **预发布版本参与 caret 匹配**:`"1.0.0-beta.0"` 会被解析到 rc.0,必须 `=` 精确锁版。
6. **esp-backtrace 的 `#[panic_handler]` 未被发现** → 显式 `extern crate esp_backtrace;`。
7. **rust-lld 1.98 不再把未定义弱引用解析为 0** → C3 的 interrupt16-31 需 `global_asm!` 空桩。
8. **ROM 符号未定义** → `.cargo/config.toml` 必须挂 `-C link-arg=-Tlinkall.x`。
9. esp-println 0.18 关 default(含 critical-section/auto)加 `esp32c3`+`log-04`;esp-backtrace 0.20 需芯片特性。
10. **实战派 C3 板已定案 flash 硬件失效**(只读不写,假擦除,三工具复现),已建议售后。
11. **Korvo-1 的 SUB3 大屏(800×480 RGB 并行)在 esp-hal 1.2.0-rc.0 上无驱动**(lcd_cam 未覆盖 S31);
    esp-rtos 0.4.0 新增的 S31 支持不含显示控制器 —— Mosaico 的 CO5300 是 GRAM 型,不依赖该控制器,因此可驱动。
