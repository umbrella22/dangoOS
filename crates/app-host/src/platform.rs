/// ESP-IDF error codes as seen by scripts (`esp_err_to_name`).
pub mod esp_err {
    pub const FAIL: i32 = -1;
    pub const NO_MEM: i32 = 0x101;
    pub const INVALID_ARG: i32 = 0x102;
    pub const INVALID_STATE: i32 = 0x103;
    pub const NOT_FOUND: i32 = 0x105;
    pub const NOT_SUPPORTED: i32 = 0x106;
    pub const TIMEOUT: i32 = 0x107;
}

/// Geometry of the screen a script draws on (RGB565 only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayInfo {
    pub width: u16,
    pub height: u16,
    pub touch_available: bool,
}

/// A rectangle of RGB565 pixels handed over by `display:present()`.
/// `pixels` starts at (x0, y0); rows are `stride` bytes apart.
pub struct Blit<'a> {
    pub x0: u16,
    pub y0: u16,
    pub x1: u16,
    pub y1: u16,
    pub pixels: &'a [u8],
    pub stride: usize,
}

impl Blit<'_> {
    pub fn width(&self) -> usize {
        (self.x1 - self.x0) as usize
    }
    pub fn height(&self) -> usize {
        (self.y1 - self.y0) as usize
    }
    /// Pixel row `r` (0-based within the rectangle) as native-endian u16s.
    pub fn row(&self, r: usize) -> impl Iterator<Item = u16> + '_ {
        let start = r * self.stride;
        self.pixels[start..start + self.width() * 2]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchPoint {
    pub id: u8,
    pub x: i32,
    pub y: i32,
}

/// `system.heap.get_info` fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct HeapInfo {
    pub total_size: u32,
    pub free_size: u32,
    pub allocated_size: u32,
    pub minimum_free_size: u32,
    pub largest_free_block: u32,
    pub allocated_blocks: u32,
    pub free_blocks: u32,
    pub total_blocks: u32,
    /// Bits in `heap_fields`; an unsupported metric is absent in Lua, even
    /// when its backing integer happens to be zero.
    pub valid_fields: u32,
    /// Minimum among explicit snapshots, not the allocator's lifetime minimum.
    pub sampled_minimum_free_size: u32,
}

pub mod heap_fields {
    pub const SIZES: u32 = 1;
    pub const MINIMUM_FREE: u32 = 2;
    pub const LARGEST_FREE: u32 = 4;
    pub const BLOCK_COUNTS: u32 = 8;
    pub const SAMPLED_MINIMUM_FREE: u32 = 16;
}

/// `heap_caps` flags as exposed in `system.heap.caps`.
pub mod caps {
    pub const SPIRAM: u32 = 1 << 10;
    pub const INTERNAL: u32 = 1 << 11;
    pub const DEFAULT: u32 = 1 << 12;
}
