//! ESP-Mosaico 开发板(乐鑫官方可扩展智能交互套件)引脚映射。
//!
//! 来源:https://docs.espressif.com/projects/esp-dev-kits/zh_CN/latest/esp32s31/esp-mosaico/user_guide.html
//! 「MCU 与管脚分配」「LCD 接口」章节(V1.0 整机)。

#![allow(dead_code)] // 引脚常量即文档,未全部用于代码是正常的

/// LCD:2.16 寸 480×480 QSPI AMOLED(CO5300 驱动,GRAM 自刷新)
pub const LCD_WIDTH: u16 = 480;
pub const LCD_HEIGHT: u16 = 480;

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
