//! ESP32-S31 开发板引脚映射。
//!
//! 支持多个硬件变体，通过 Cargo Features 选择：
//! - `mosaico`: ESP-Mosaico 官方开发板 (480×480 AMOLED)
//! - `korvo`: ESP32-S31-Korvo-1 开发板 (800×480 RGB LCD)
//!
//! 来源:
//! - Mosaico: https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s31/esp-mosaico/user_guide.html
//! - Korvo-1: https://github.com/espressif/esp-bsp/tree/master/bsp/esp32_s3_korvo_1
//! - LCD Sub3: https://github.com/espressif/esp-bsp/tree/master/bsp/esp32_s3_lcd_ev_board

#![allow(dead_code)] // 引脚常量即文档,未全部用于代码是正常的

// ============================================
// 变体: ESP-Mosaico 官方开发板
// ============================================
#[cfg(feature = "mosaico")]
pub mod mosaico {
    /// LCD:2.16 寸 480×480 QSPI AMOLED(CO5300 驱动,GRAM 自刷新)
    pub const LCD_WIDTH: u16 = 480;
    pub const LCD_HEIGHT: u16 = 480;
    
    /// LCD 类型标识
    pub const LCD_TYPE: &str = "QSPI-AMOLED-480x480";
    pub const LCD_INTERFACE: &str = "QSPI";
    
    /// QSPI 时钟频率(Hz):官方 BSP 用 40MHz
    pub const LCD_SPI_HZ: u32 = 40_000_000;
    
    /// LCD QSPI 引脚
    pub const LCD_SCLK_GPIO: u8 = 44;
    pub const LCD_DATA0_GPIO: u8 = 36;
    pub const LCD_DATA1_GPIO: u8 = 51;
    pub const LCD_DATA2_GPIO: u8 = 35;
    pub const LCD_DATA3_GPIO: u8 = 9;
    pub const LCD_CS_GPIO: u8 = 50;
    pub const LCD_RST_GPIO: u8 = 42;
    pub const LCD_TE_GPIO: u8 = 43;
    
    /// 状态 LED(橙色,低电平点亮)
    pub const STATUS_LED_GPIO: u8 = 3;
    
    /// AI 按键(低电平有效)
    pub const AI_BUTTON_GPIO: u8 = 7;
    
    /// 振动马达(高电平开启)
    pub const MOTOR_GPIO: u8 = 8;
    
    /// 触摸中断(CST9220 @ I2C 0x5A)
    pub const TOUCH_INT_GPIO: u8 = 6;
    
    /// 风扇 PWM 候选引脚:模块扩展座 H2 的 GPIO12
    pub const FAN_PWM_GPIO: u8 = 12;
    
    /// 3.3V 电源控制引脚
    pub const VCC_3V3_CTRL_GPIO: u8 = 60;
    
    /// 硬件特性标识（用于条件编译）
    pub const HAS_RGB_LED: bool = false;
    pub const HAS_AUDIO: bool = false;
    pub const HAS_CAMERA: bool = false;
}

// ============================================
// 变体: ESP32-S31-Korvo-1 开发板
// ============================================
#[cfg(feature = "korvo")]
pub mod korvo {
    /// LCD:4.3 寸 800×480 RGB LCD (ESP32-S3-LCD-EV-Board-SUB3)
    pub const LCD_WIDTH: u16 = 800;
    pub const LCD_HEIGHT: u16 = 480;
    
    /// LCD 类型标识
    pub const LCD_TYPE: &str = "RGB-800x480";
    pub const LCD_INTERFACE: &str = "RGB";  // RGB 并口接口
    
    /// LCD RGB 接口引脚（来自官方 BSP）
    pub const LCD_PCLK_GPIO: u8 = 9;    // Pixel Clock
    pub const LCD_HSYNC_GPIO: u8 = 46;  // H-Sync
    pub const LCD_VSYNC_GPIO: u8 = 3;   // V-Sync
    pub const LCD_DE_GPIO: u8 = 17;     // Data Enable
    
    /// RGB 数据线（16-bit RGB565）
    pub const LCD_DATA0_GPIO: u8 = 10;   // Red[0]
    pub const LCD_DATA1_GPIO: u8 = 11;   // Red[1]
    pub const LCD_DATA2_GPIO: u8 = 12;   // Red[2]
    pub const LCD_DATA3_GPIO: u8 = 13;   // Red[3]
    pub const LCD_DATA4_GPIO: u8 = 14;   // Red[4]
    
    pub const LCD_DATA5_GPIO: u8 = 21;   // Green[0]
    pub const LCD_DATA6_GPIO: u8 = 47;   // Green[1]
    pub const LCD_DATA7_GPIO: u8 = 48;   // Green[2]
    pub const LCD_DATA8_GPIO: u8 = 45;   // Green[3]
    pub const LCD_DATA9_GPIO: u8 = 38;   // Green[4]
    pub const LCD_DATA10_GPIO: u8 = 39;  // Green[5]
    
    pub const LCD_DATA11_GPIO: u8 = 40;  // Blue[0]
    pub const LCD_DATA12_GPIO: u8 = 41;  // Blue[1]
    pub const LCD_DATA13_GPIO: u8 = 42;  // Blue[2]
    pub const LCD_DATA14_GPIO: u8 = 2;   // Blue[3]
    pub const LCD_DATA15_GPIO: u8 = 1;   // Blue[4]
    
    /// 背光控制（需要通过 IO Expander）
    pub const LCD_BL_CTRL_VIA_EXPANDER: bool = true;
    
    /// I2C 引脚（主板配置，用于 IO Expander）
    pub const I2C_SCL_GPIO: u8 = 2;
    pub const I2C_SDA_GPIO: u8 = 1;
    
    /// 可寻址 RGB LED (WS2812 x12)
    pub const RGB_LED_GPIO: u8 = 19;     // 官方 BSP 定义
    pub const RGB_LED_NUM: u8 = 12;
    
    /// 按键（所有按键共用一个 GPIO，通过 ADC 区分）
    pub const BUTTONS_GPIO: u8 = 8;
    
    /// 功放电源控制
    pub const POWER_AMP_GPIO: u8 = 38;
    
    /// 风扇 PWM 引脚（使用空闲 GPIO）
    pub const FAN_PWM_GPIO: u8 = 4;   // GPIO4 空闲可用
    
    /// 硬件特性标识（用于条件编译）
    pub const HAS_RGB_LED: bool = true;
    pub const HAS_AUDIO: bool = true;   // I2S 音频
    pub const HAS_CAMERA: bool = true;  // DVP 摄像头接口
}

// ============================================
// 公共接口：根据编译时 feature 导出对应变体
// ============================================
#[cfg(feature = "mosaico")]
pub use mosaico::*;

#[cfg(feature = "korvo")]
pub use korvo::*;

// 默认配置（桌面开发或未指定变体时）
#[cfg(not(any(feature = "mosaico", feature = "korvo")))]
pub use mosaico::*;  // 桌面开发默认使用 Mosaico 的配置

// ============================================
// 通用配置（所有变体共享）
// ============================================

/// 帧缓冲字节数 - 根据编译时的屏幕尺寸计算
#[cfg(feature = "mosaico")]
pub const LCD_FB_BYTES: usize = 480 * 480 * 2;  // RGB565: 450KB

#[cfg(feature = "korvo")]
pub const LCD_FB_BYTES: usize = 800 * 480 * 2;  // RGB565: 750KB

#[cfg(not(any(feature = "mosaico", feature = "korvo")))]
pub const LCD_FB_BYTES: usize = 480 * 480 * 2;  // 默认

/// 目标帧时间(毫秒):60fps
pub const TARGET_FRAME_TIME_MS: u64 = 16;

/// 风扇 PWM 频率(Hz):典型 PC 风扇 25kHz
pub const FAN_PWM_FREQ_HZ: u32 = 25_000;

/// 风扇 PWM 最大占空比(百分比)
pub const FAN_PWM_MAX_DUTY: u8 = 100;

/// 看门狗超时时间(毫秒):主循环挂死保护
pub const WATCHDOG_TIMEOUT_MS: u64 = 3000;

// === CO5300 寄存器地址(补充文档) ===

/// 列地址设置(CASET)
pub const CO5300_REG_CASET: u8 = 0x2A;

/// 行地址设置(RASET)  
pub const CO5300_REG_RASET: u8 = 0x2B;

/// 写 GRAM(RAMWR)
pub const CO5300_REG_RAMWR: u8 = 0x2C;

/// 亮度寄存器(0x00..0xFF)
pub const CO5300_REG_BRIGHTNESS: u8 = 0x51;

/// 官方默认窗口 X 起始
pub const CO5300_DEFAULT_WINDOW_X_START: u16 = 0x0006;
/// 官方默认窗口 X 结束
pub const CO5300_DEFAULT_WINDOW_X_END: u16 = 0x01DD;
/// 官方默认窗口 Y 起始
pub const CO5300_DEFAULT_WINDOW_Y_START: u16 = 0x0000;
/// 官方默认窗口 Y 结束
pub const CO5300_DEFAULT_WINDOW_Y_END: u16 = 0x01D1;

/// 帧缓冲字节数(RGB565:480×480×2 ≈ 450KB,放 PSRAM)
pub const LCD_FB_BYTES: usize = (LCD_WIDTH as usize) * (LCD_HEIGHT as usize) * 2;

/// QSPI 时钟频率(Hz):官方 BSP 用 40MHz
pub const LCD_SPI_HZ: u32 = 40_000_000;

/// LCD QSPI 引脚
pub const LCD_SCLK_GPIO: u8 = 44;
pub const LCD_DATA0_GPIO: u8 = 36;
pub const LCD_DATA1_GPIO: u8 = 51;
pub const LCD_DATA2_GPIO: u8 = 35;
pub const LCD_DATA3_GPIO: u8 = 9;
pub const LCD_CS_GPIO: u8 = 50;
pub const LCD_RST_GPIO: u8 = 42;
/// TE 防撕裂同步(当前未用)
pub const LCD_TE_GPIO: u8 = 43;

/// 状态 LED(橙色,低电平点亮)
pub const STATUS_LED_GPIO: u8 = 3;

/// AI 按键(低电平有效)
pub const AI_BUTTON_GPIO: u8 = 7;

/// 振动马达(高电平开启)
pub const MOTOR_GPIO: u8 = 8;

/// 触摸中断(CST9220 @ I2C 0x5A)
pub const TOUCH_INT_GPIO: u8 = 6;

/// 风扇 PWM 候选引脚:模块扩展座 H2 的 GPIO12(空闲脚)
pub const FAN_PWM_GPIO: u8 = 12;

/// 3.3V 电源控制引脚(高电平开启外设电源轨)
pub const VCC_3V3_CTRL_GPIO: u8 = 60;

// === 性能与时序常量 ===

/// 目标帧时间(毫秒):60fps
pub const TARGET_FRAME_TIME_MS: u64 = 16;

/// 风扇 PWM 频率(Hz):典型 PC 风扇 25kHz
pub const FAN_PWM_FREQ_HZ: u32 = 25_000;

/// 风扇 PWM 最大占空比(百分比)
pub const FAN_PWM_MAX_DUTY: u8 = 100;

/// 看门狗超时时间(毫秒):主循环挂死保护
pub const WATCHDOG_TIMEOUT_MS: u64 = 3000;

// === CO5300 寄存器地址(补充文档) ===

/// 列地址设置(CASET)
pub const CO5300_REG_CASET: u8 = 0x2A;

/// 行地址设置(RASET)  
pub const CO5300_REG_RASET: u8 = 0x2B;

/// 写 GRAM(RAMWR)
pub const CO5300_REG_RAMWR: u8 = 0x2C;

/// 亮度寄存器(0x00..0xFF)
pub const CO5300_REG_BRIGHTNESS: u8 = 0x51;

/// 官方默认窗口 X 起始
pub const CO5300_DEFAULT_WINDOW_X_START: u16 = 0x0006;
/// 官方默认窗口 X 结束
pub const CO5300_DEFAULT_WINDOW_X_END: u16 = 0x01DD;
/// 官方默认窗口 Y 起始
pub const CO5300_DEFAULT_WINDOW_Y_START: u16 = 0x0000;
/// 官方默认窗口 Y 结束
pub const CO5300_DEFAULT_WINDOW_Y_END: u16 = 0x01D1;
