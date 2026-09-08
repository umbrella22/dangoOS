//! ESP32-S31 开发板引脚映射。
//!
//! 支持多个硬件变体，通过 Cargo Features 选择：
//! - `s31-variant-a`: ESP-Mosaico 官方开发板
//! - `s31-variant-b`: 自定义 S31 配置
//!
//! 来源:
//! - Variant A: https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s31/esp-mosaico/user_guide.html

#![allow(dead_code)] // 引脚常量即文档,未全部用于代码是正常的

// ============================================
// 变体 A: ESP-Mosaico 官方开发板
// ============================================
#[cfg(feature = "s31-variant-a")]
pub mod variant_a {
    /// LCD:2.16 寸 480×480 QSPI AMOLED(CO5300 驱动,GRAM 自刷新)
    pub const LCD_WIDTH: u16 = 480;
    pub const LCD_HEIGHT: u16 = 480;
    
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
}

// ============================================
// 变体 B: 自定义 S31 配置
// ============================================
#[cfg(feature = "s31-variant-b")]
pub mod variant_b {
    /// LCD:2.16 寸 480×480 QSPI AMOLED
    pub const LCD_WIDTH: u16 = 480;
    pub const LCD_HEIGHT: u16 = 480;
    
    /// QSPI 时钟频率(Hz) - 可能需要调整
    pub const LCD_SPI_HZ: u32 = 40_000_000;
    
    /// LCD QSPI 引脚 - 根据实际硬件修改
    pub const LCD_SCLK_GPIO: u8 = 44;
    pub const LCD_DATA0_GPIO: u8 = 36;
    pub const LCD_DATA1_GPIO: u8 = 51;
    pub const LCD_DATA2_GPIO: u8 = 35;
    pub const LCD_DATA3_GPIO: u8 = 9;
    pub const LCD_CS_GPIO: u8 = 50;
    pub const LCD_RST_GPIO: u8 = 42;
    pub const LCD_TE_GPIO: u8 = 43;
    
    /// 状态 LED - 根据实际硬件修改
    pub const STATUS_LED_GPIO: u8 = 10;  // 示例：不同的引脚
    
    /// 风扇 PWM 引脚 - 根据实际硬件修改
    pub const FAN_PWM_GPIO: u8 = 15;  // 示例：不同的引脚
    
    /// 3.3V 电源控制引脚
    pub const VCC_3V3_CTRL_GPIO: u8 = 60;
}

// ============================================
// 公共接口：根据编译时 feature 导出对应变体
// ============================================
#[cfg(feature = "s31-variant-a")]
pub use variant_a::*;

#[cfg(feature = "s31-variant-b")]
pub use variant_b::*;

// 默认配置（桌面开发或未指定变体时）
#[cfg(not(any(feature = "s31-variant-a", feature = "s31-variant-b")))]
pub use default_config::*;

// ============================================
// 通用配置（所有变体共享）
// ============================================

/// 帧缓冲字节数(RGB565:480×480×2 ≈ 450KB,放 PSRAM)
pub const LCD_FB_BYTES: usize = 480 * 480 * 2;

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
