//! ESP-Mosaico v1.2 板级常量（从 fan-firmware/board.rs 裁剪，来源注释保留）。
//!
//! ⚠️ v1.0 与 v1.2 引脚差异巨大（LCD SCL/RST 互换、主 I2C 挪到 56/3）。
//! 本固件按 v1.2 布线，启动时从 eFuse 校验硬件版本。

#![allow(dead_code)] // 引脚常量即文档

/// 板硬件版本，从 eFuse USER_DATA 前 16 位读出 (major<<8 | minor)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareVersion {
    pub major: u8,
    pub minor: u8,
}

/// 读 eFuse USER_DATA 并解析硬件版本；Err 携带不支持的原始版本。
pub fn board_variant() -> Result<(u8, u8), HardwareVersion> {
    let raw: u16 = esp_hal::efuse::read_field_le(esp_hal::efuse::BLOCK_USR_DATA);
    let v = HardwareVersion {
        major: (raw >> 8) as u8,
        minor: (raw & 0xFF) as u8,
    };
    match (v.major, v.minor) {
        (1, 1) | (1, 2) => Ok((v.major, v.minor)),
        _ => Err(v),
    }
}

// ---- CO5300 QSPI AMOLED 480×480 ----
pub const LCD_WIDTH: u16 = 480;
pub const LCD_HEIGHT: u16 = 480;
pub const LCD_FB_PIXELS: usize = 480 * 480;
pub const LCD_FB_BYTES: usize = LCD_FB_PIXELS * 2; // RGB565

/// QSPI 像素时钟 40MHz（官方 BSP 默认）
pub const LCD_PIXEL_CLOCK_HZ: u32 = 40_000_000;

pub const LCD_CS_GPIO: u8 = 50;
pub const LCD_DATA0_GPIO: u8 = 36;
pub const LCD_DATA1_GPIO: u8 = 51;
pub const LCD_DATA2_GPIO: u8 = 35;
pub const LCD_DATA3_GPIO: u8 = 9;
/// v1.2: SCL=42 / RST=44（与 v1.0 的 44/42 **互换**！）
pub const LCD_SCL_GPIO_V1_2: u8 = 42;
pub const LCD_RST_GPIO_V1_2: u8 = 44;

// ---- 触摸 CST9220（I2C）----
pub const TOUCH_I2C_ADDR: u8 = 0x5A; // 7-bit；线上写 0xB4 / 读 0xB5

// ---- I2C 总线（v1.2: 主板 I2C0 SDA=56 / SCL=3）----
pub const I2C_SDA_GPIO_V1_2: u8 = 56;
pub const I2C_SCL_GPIO_V1_2: u8 = 3;

// ---- 电源轨 ----
/// VCC_3V3 总闸，**低电平有效**；显示/触摸上电前必须先开它
pub const POWER_VCC_3V3_GPIO: u8 = 60;
