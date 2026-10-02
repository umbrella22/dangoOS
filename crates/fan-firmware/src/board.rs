//! ESP-Mosaico 板级常量（硬件 v1.2 实测板）
//!
//! 来源: 官方 BSP `esp-mosaico/esp-mosaico-bsp` v0.2.0
//! https://github.com/esp-mosaico/esp-mosaico-bsp（本地克隆: ~/Downloads/esp-mosaico-bsp）
//! 完整硬件真相与移植注记见 `docs/MOSAICO_V12_HARDWARE.md`。
//!
//! ⚠️ v1.0 与 v1.2 引脚差异巨大（LCD SCL/RST 互换、主 I2C 挪到 56/3）。
//! 本文件以 v1.2 为主，v1.0 仅作对照保留。硬件版本运行时从 eFuse 读取。

#![allow(dead_code)] // 引脚常量即文档

// ---- 硬件版本检测（eFuse USER_DATA 块前 16 位: major<<8 | minor）----
// 官方规则: v1.0 → V1_0; v1.1/v1.2 → 按 V1_2 处理; 其他版本拒绝启动。
// v1.1 与 v1.2 引脚相同。

/// 板硬件版本，从 eFuse `USER_DATA` 读出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareVersion {
    pub major: u8,
    pub minor: u8,
}

/// 板代际。v1.1 出厂时按 v1.2 的引脚接线，官方 BSP 将两者归为同一变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardVariant {
    V1_0,
    V1_2,
}

/// 读 eFuse USER_DATA 前 16 位并解析硬件版本。
///
/// USER_DATA 字段共 192 位，官方 BSP 只用前 16 位存 `major<<8 | minor`。
pub fn hardware_version() -> HardwareVersion {
    let raw: u16 = esp_hal::efuse::read_field_le(esp_hal::efuse::BLOCK_USR_DATA);
    HardwareVersion {
        major: (raw >> 8) as u8,
        minor: (raw & 0xFF) as u8,
    }
}

/// 解析板代际。`Err` 携带不支持的原始版本（官方 BSP 此时直接拒绝启动）。
pub fn board_variant() -> Result<BoardVariant, HardwareVersion> {
    let v = hardware_version();
    match (v.major, v.minor) {
        (1, 0) => Ok(BoardVariant::V1_0),
        (1, 1) | (1, 2) => Ok(BoardVariant::V1_2),
        _ => Err(v),
    }
}

// ---- CO5300 QSPI AMOLED 480×480（v1.0/v1.2 仅 SCL/RST 不同）----
pub const LCD_WIDTH: u16 = 480;
pub const LCD_HEIGHT: u16 = 480;
pub const LCD_FB_PIXELS: usize = 480 * 480;
pub const LCD_FB_BYTES: usize = LCD_FB_PIXELS * 2; // RGB565

/// QSPI 像素时钟 40MHz（官方 BSP 默认）
pub const LCD_PIXEL_CLOCK_HZ: u32 = 40_000_000;
/// 面板窗口无 gap（0x2A/0x2B 全窗 0..479）
pub const LCD_X_GAP: u16 = 0;
pub const LCD_Y_GAP: u16 = 0;

pub const LCD_CS_GPIO: u8 = 50;
pub const LCD_TE_GPIO: u8 = 43; // TE 可选防撕裂（官方 BSP 有 TE 同步模式）
pub const LCD_DATA0_GPIO: u8 = 36;
pub const LCD_DATA1_GPIO: u8 = 51;
pub const LCD_DATA2_GPIO: u8 = 35;
pub const LCD_DATA3_GPIO: u8 = 9;
/// v1.2: SCL=42 / RST=44（与 v1.0 的 44/42 **互换**！烧录前必须确认版本）
pub const LCD_SCL_GPIO_V1_2: u8 = 42;
pub const LCD_RST_GPIO_V1_2: u8 = 44;
pub const LCD_SCL_GPIO_V1_0: u8 = 44;
pub const LCD_RST_GPIO_V1_0: u8 = 42;

/// 官方 BSP 把 QSPI 传输按 8 行一块切 DMA；S31 LCD_CAM 不能直读 PSRAM
/// （`psram_dma_direct = false`），与我们 Korvo-1 上的结论一致。
pub const LCD_DMA_CHUNK_LINES: usize = 8;

/// QSPI 脏区约束: X 坐标必须 4 像素对齐（4-lane 并行一次 4 像素）。
/// Slint 脏区/任何局部重绘的 x1 向下取整到 4 的倍数、x2 向上补齐。
pub const LCD_QSPI_X_ALIGN: usize = 4;

// ---- CO5300 寄存器（从官方 BSP esp_lcd_co5300 ^2.2 + bsp display.c 提取）----
pub mod co5300 {
    /// 厂商初始化命令: (寄存器, 参数, 延时 ms)
    pub struct LcdCmd {
        pub reg: u8,
        pub data: &'static [u8],
        pub delay_ms: u32,
    }

    /// 完整上电初始化（冷启动，含两次 600ms 延时）
    /// TE(0x35) 可选；官方 BSP 用 Kconfig 开关控制，这里按启用 TE 保留。
    pub const VENDOR_INIT: &[LcdCmd] = &[
        LcdCmd {
            reg: 0x11,
            data: &[],
            delay_ms: 600,
        }, // SLPOUT
        LcdCmd {
            reg: 0xFE,
            data: &[0x20],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x19,
            data: &[0x10],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x1C,
            data: &[0xA0],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0xFE,
            data: &[0x00],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0xC4,
            data: &[0x80],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x3A,
            data: &[0x55],
            delay_ms: 0,
        }, // RGB565
        LcdCmd {
            reg: 0x35,
            data: &[0x00],
            delay_ms: 0,
        }, // TE on
        LcdCmd {
            reg: 0x53,
            data: &[0x20],
            delay_ms: 0,
        }, // BCTRL 使能亮度控制
        LcdCmd {
            reg: 0x51,
            data: &[0xFF],
            delay_ms: 0,
        }, // 亮度 = 255
        LcdCmd {
            reg: 0x63,
            data: &[0xFF],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x2A,
            data: &[0x00, 0x00, 0x01, 0xDF],
            delay_ms: 0,
        }, // 列 0..479
        LcdCmd {
            reg: 0x2B,
            data: &[0x00, 0x00, 0x01, 0xDF],
            delay_ms: 0,
        }, // 行 0..479
        LcdCmd {
            reg: 0x29,
            data: &[],
            delay_ms: 600,
        }, // DISPON
    ];

    /// Boot splash 交接初始化（bootloader 已完成复位/SLPOUT/DISPON 时，
    /// 跳过 0x11/0x51/0x29，避免重放延时命令把开机画面打黑 ~1.2s）
    pub const HANDOFF_INIT: &[LcdCmd] = &[
        LcdCmd {
            reg: 0xFE,
            data: &[0x20],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x19,
            data: &[0x10],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x1C,
            data: &[0xA0],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0xFE,
            data: &[0x00],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0xC4,
            data: &[0x80],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x3A,
            data: &[0x55],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x35,
            data: &[0x00],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x53,
            data: &[0x20],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x63,
            data: &[0xFF],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x2A,
            data: &[0x00, 0x00, 0x01, 0xDF],
            delay_ms: 0,
        },
        LcdCmd {
            reg: 0x2B,
            data: &[0x00, 0x00, 0x01, 0xDF],
            delay_ms: 0,
        },
    ];

    // 状态控制寄存器
    pub const REG_MADCTL: u8 = 0x36; // swap_xy/mirror 全走这里
    pub const REG_TE_ON: u8 = 0x35;
    pub const REG_BRIGHTNESS: u8 = 0x51; // 0-255；官方 API 按百分比×255 映射
    pub const REG_WR_CTRL_DISPLAY: u8 = 0x53; // 0x20 = BCTRL（亮度控制必须先开）
    pub const REG_SLEEP_IN: u8 = 0x10;
    pub const REG_SLEEP_OUT: u8 = 0x11;
    pub const REG_DISPLAY_OFF: u8 = 0x28;
    pub const REG_DISPLAY_ON: u8 = 0x29;
    pub const REG_DEEP_STANDBY: u8 = 0x4F; // DSTBON：退出需 RESX 硬复位+全量重初始化
}

// ---- 触摸 CST9220（I2C）----
/// 触摸中断，低有效；无复位脚
pub const TOUCH_INT_GPIO: u8 = 6;
/// 7-bit HAL 地址。官方 board_devices.yaml 使用 8-bit 写地址 0xB4。
/// HAL 自行添加读写位：线上写地址 0xB4、读地址 0xB5。
pub const TOUCH_I2C_ADDR: u8 = 0x5A;
pub const TOUCH_I2C_HZ: u32 = 400_000;
/// I2C 事务超时（官方注释: 留 0 会死等，且死等发生在 LVGL 锁内）
pub const TOUCH_I2C_TIMEOUT_MS: u32 = 100;

// ---- I2C 总线（v1.2 变化最大的一条）----
/// 主板 I2C0: v1.0 在 GPIO0/1，**v1.2 挪到 SDA=56 / SCL=3**
pub const I2C_SDA_GPIO_V1_2: u8 = 56;
pub const I2C_SCL_GPIO_V1_2: u8 = 3;
pub const I2C_SDA_GPIO_V1_0: u8 = 0;
pub const I2C_SCL_GPIO_V1_0: u8 = 1;
/// 扩展槽 I2C: v1.2 专用 I2C1（SDA=0/SCL=1）；v1.0 与主板共享 I2C0
pub const SUBBOARD_I2C_SDA_GPIO: u8 = 0;
pub const SUBBOARD_I2C_SCL_GPIO: u8 = 1;

// ---- 传感器地址（全部挂主板 I2C，400kHz）----
pub const IMU_BMI270_ADDR: u8 = 0x69;
pub const MAG_BMM150_ADDR_0: u8 = 0x11;
pub const MAG_BMM150_ADDR_1: u8 = 0x12;
pub const BATTERY_BQ27220_ADDR: u8 = 0x55; // 7-bit
/// 传感器共享中断（板上二极管线或），低有效
pub const SENSOR_INT_GPIO: u8 = 2;

// ---- 音频 ES8311 ----
pub const AUDIO_I2S_MCLK_GPIO: u8 = 54;
pub const AUDIO_I2S_BCLK_GPIO: u8 = 37;
pub const AUDIO_I2S_WS_GPIO: u8 = 49;
pub const AUDIO_I2S_DOUT_GPIO: u8 = 52;
pub const AUDIO_I2S_DIN_GPIO: u8 = 40;
/// 功放使能
pub const AUDIO_PA_CTRL_GPIO: u8 = 45;
/// ES8311 7-bit 地址（原理图 0x19）
pub const AUDIO_CODEC_ADDR_7BIT: u8 = 0x19;

// ---- 电源轨 ----
/// VCC_3V3 总闸，**低电平有效**；显示/触摸/传感器上电前必须先开它
pub const POWER_VCC_3V3_GPIO: u8 = 60;
pub const POWER_VCC_3V3_ON_LEVEL: u8 = 0;
pub const POWER_VCC_3V3_OFF_LEVEL: u8 = 1;
/// 编解码器电源: 仅 v1.0 存在（GPIO56 高有效）；**v1.2 常供电无控制脚**
/// （GPIO56 在 v1.2 上让给了主板 I2C SDA）
pub const POWER_CODEC_3V3_GPIO_V1_0: u8 = 56;
/// 关机信号，开漏输出，拉低 = 请求断电
pub const POWER_SWITCH_GPIO: u8 = 57;
pub const POWER_SWITCH_ASSERT_LEVEL: u8 = 0;

// ---- 按键 / 马达 / LED ----
pub const BUTTON_AI_GPIO: u8 = 7; // 低有效
pub const BUTTON_BOOT_GPIO: u8 = 61;
/// 震动马达，高有效（官方 BSP 可选 5kHz PWM 调强度）
pub const MOTOR_GPIO: u8 = 8;
/// 状态 LED 仅 v1.0 存在（GPIO3）；**v1.2 上 GPIO3 = 主板 I2C SCL，无 LED**
pub const LED_STATUS_GPIO_V1_0: u8 = 3;

// ---- 风扇 PWM ----
/// 风扇 PWM 候选引脚: 模块座 GPIO12。
/// ⚠️ GPIO12 同时是**左槽 H8**（摇杆按钮3 / 交互模块 KEY_R）——
/// 「手持控制器 + 左槽插模块」场景必须仲裁: 要么换脚，要么运行时确认
/// 左槽无模块后再启用风扇 PWM（官方 module_mgr 提供 claim/release 机制）。
pub const FAN_PWM_GPIO: u8 = 12;
/// 风扇 PWM 载波频率（PC 风扇标准 25kHz, RMT 连续发送实现）
pub const FAN_PWM_HZ: u32 = 25_000;

// ---- SPI NAND flash（走 SD 焊盘复用，SPI3_HOST）----
pub const NAND_CLK_GPIO: u8 = 20;
pub const NAND_D_GPIO: u8 = 21;
pub const NAND_Q_GPIO: u8 = 22;
pub const NAND_CS_GPIO: u8 = 23;
pub const NAND_HOLD_GPIO: u8 = 24;
pub const NAND_WP_GPIO: u8 = 25;

// ---- PPA 2D 加速器（PAC 0.3.0 已暴露，esp-hal 尚无安全驱动）----
/// S31 PPA 基地址（esp32s31 PAC: `PPA = Periph<ppa::RegisterBlock, 0x2034_5000>`，中断 102）。
/// 能力: 双层 blend（色键/固定 alpha/CLUT）+ SRM 缩放旋转镜像 + RGB2GRAY。
/// esp-hal rev bb239a0f 没有驱动；可用 PAC 直写寄存器，或等官方驱动落地。
pub const PPA_BASE: usize = 0x2034_5000;
pub const PPA_INTERRUPT: u32 = 102;

// ---- 背部磁吸扩展槽（subboard，官方 BSP `subboard.c` 定义）----
// v1.2: 专用 I2C1（SDA=0/SCL=1）。v1.0 与主板共享 I2C0。
// 模块身份 = 槽上 AT24C02 EEPROM（描述符 0x86B, magic "ESP"），
// 槽位地址选择脚接 EEPROM 的 A0:
//   左槽 GPIO14=0 → EEPROM@0x50；右槽 GPIO39=1 → EEPROM@0x51。
// 相机模块例外: EEPROM 固定 0x50 无地址线，GPIO14 复用为相机 D4。
pub mod subboard {
    /// 槽位枚举的 usize 值（0=左 1=右），与官方 bsp_subboard_slot_t 一致
    pub const SLOT_LEFT: usize = 0;
    pub const SLOT_RIGHT: usize = 1;

    /// 左槽 6 针连接器（H2/H4/H6/H8/H10/H12 顺序，官方 canonical 左槽编号）
    pub const LEFT_CONNECTOR: [u8; 6] = [53, 48, 13, 12, 14, 4];
    /// 右槽 6 针连接器（180° 镜像布线）
    pub const RIGHT_CONNECTOR: [u8; 6] = [46, 47, 11, 10, 39, 5];

    /// EEPROM 7-bit 地址与槽位地址选择
    pub const EEPROM_ADDR_LEFT: u8 = 0x50;
    pub const EEPROM_ADDR_RIGHT: u8 = 0x51;
    pub const ADDR_SELECT_GPIO_LEFT: u8 = 14; // 拉低 → 左槽 EEPROM@0x50
    pub const ADDR_SELECT_GPIO_RIGHT: u8 = 39; // 拉高 → 右槽 EEPROM@0x51

    /// 左槽 canonical GPIO ↔ 右槽镜像对（6 针连接器 + 6 对磁吸延长触点）。
    /// 注意延长对包含完整音频 I2S 总线: 17↔37(BCLK) 55↔49(LRCLK)
    /// 19↔52(DOUT) 18↔54(MCLK) 16↔40(DIN) —— 右槽触点直通板上音频。
    pub const GPIO_PAIRS: [(u8, u8); 12] = [
        (53, 46), // H2
        (48, 47), // H4
        (13, 11), // H6
        (12, 10), // H8
        (14, 39), // H10 = 标准模块 EEPROM A0
        (4, 5),   // H12
        (16, 40), // 延长对: 左16 = 音频 DIN / 右40
        (15, 38), // 延长对
        (17, 37), // 延长对: 左17 / 右37 = 音频 BCLK
        (18, 54), // 延长对: 左18 / 右54 = 音频 MCLK
        (19, 52), // 延长对: 左19 / 右52 = 音频 DOUT
        (55, 49), // 延长对: 左55 / 右49 = 音频 LRCLK
    ];

    /// EEPROM 描述符 board_type（官方 mosaico_board_type_t）——模块生态规划
    pub mod board_type {
        pub const CORE: u8 = 0x01;
        pub const POWER: u8 = 0x02;
        pub const DOCK: u8 = 0x03;
        pub const HANDLE: u8 = 0x04; // 摇杆手柄
        pub const BALANCE_CAR: u8 = 0x05;
        pub const DISPLAY: u8 = 0x06;
        pub const CAMERA: u8 = 0x07;
        pub const SENSOR: u8 = 0x08;
        pub const IO_EXP: u8 = 0x09;
        pub const TOF: u8 = 0x10;
        pub const MATRIX_LED: u8 = 0x11;
        pub const THERMAL: u8 = 0x12;
        pub const RELAY: u8 = 0x13;
        pub const INTERACT: u8 = 0x16;
    }

    // ---- 官方三模块的 canonical（左槽）引脚占用 ----
    /// 摇杆模块(HANDLE): X/Y 轴走 ADC oneshot；B 键在延长触点上
    pub mod joystick {
        pub const AXIS_X_GPIO: u8 = 48; // H4, ADC
        pub const AXIS_Y_GPIO: u8 = 53; // H2, ADC
        /// B/1/2/3/4（官方驱动定义: B 带 true 标志=上拉, 其余低有效）
        pub const BUTTONS: [u8; 5] = [16, 4, 15, 12, 13];
    }
    /// 交互模块(INTERACT): 光敏/红外/双键/人体感应/WS2812
    pub mod interact {
        pub const LDR_GPIO: u8 = 53; // H2 光敏电阻
        pub const IR_GPIO: u8 = 48; // H4 红外发射（RMT NEC）
        pub const KEY_L_GPIO: u8 = 13; // H6
        pub const KEY_R_GPIO: u8 = 12; // H8
        pub const PIR_GPIO: u8 = 4; // H12 人体感应
        pub const WS2812_GPIO: u8 = 15; // 延长触点
    }
    /// 相机模块(CAMERA): **仅左槽**（EEPROM 固定 0x50, GPIO14=D4）
    pub mod camera {
        /// DVP 8-bit 数据线 D0..D7（左槽实脚）
        pub const DATA_GPIOS: [u8; 8] = [16, 15, 33, 4, 14, 12, 18, 13];
        pub const VSYNC_GPIO: u8 = 55;
        pub const HREF_GPIO: u8 = 19;
        pub const PCLK_GPIO: u8 = 17;
        pub const RESET_GPIO: u8 = 53;
        pub const PWDN_GPIO: u8 = 48;
        /// XCLK = NC: 模块自带时钟源
        /// 闪光灯: 主机侧 GPIO34, 开漏输出, **低电平点亮**, 初始化即强制关
        pub const FLASH_HOST_GPIO: u8 = 34;
        pub const EEPROM_ADDR: u8 = 0x50;
    }
}
