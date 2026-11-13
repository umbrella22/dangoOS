//! Shared visual tokens for the native pages.

use rs_lvgl_ui::Color;

pub const BG: Color = Color::rgb(0x00, 0x00, 0x00);
pub const CARD: Color = Color::rgb(0x18, 0x18, 0x19);
pub const CARD_HI: Color = Color::rgb(0x28, 0x28, 0x29);
pub const TEXT: Color = Color::rgb(0xFC, 0xFC, 0xFF);
pub const TITLE: Color = Color::rgb(0xD6, 0xD6, 0xDE);
pub const MUTED: Color = Color::rgb(0x91, 0x91, 0x9B);
pub const WEAK: Color = Color::rgb(0x59, 0x59, 0x59);
pub const ACCENT: Color = Color::rgb(0xFF, 0x4C, 0x01);
pub const GREEN: Color = Color::rgb(0x63, 0xC2, 0x70);
pub const YELLOW: Color = Color::rgb(0xFF, 0xB0, 0x20);
pub const BLUE: Color = Color::rgb(0x3D, 0x8B, 0xFF);

/// Width of the content column.
pub const COLUMN_W: i16 = 400;
/// Top of the content, below the title.
pub const CONTENT_Y: i16 = 92;
pub const ROW_H: i16 = 68;
pub const RADIUS: i16 = 20;
