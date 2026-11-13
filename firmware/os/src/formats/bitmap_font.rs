//! MFNT v1, packed rows of 4-bit pixels. Parse before handing data to LVGL.
const HEADER: usize = 16;
const ENTRY: usize = 16;

#[derive(Clone, Copy)]
pub struct FontData<'a>(&'a [u8]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub index: u32,
    pub offset: u32,
    pub adv: u16,
    pub w: u8,
    pub h: u8,
    pub ofs_x: i8,
    pub ofs_y: i8,
}

const fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}
const fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}
impl<'a> FontData<'a> {
    /// # Safety
    /// `d` must have passed `parse` and remain immutable for this view's lifetime.
    pub(crate) unsafe fn from_validated(d: &'a [u8]) -> Self {
        Self(d)
    }
    pub const fn parse(d: &'a [u8]) -> Result<Self, &'static str> {
        if d.len() < HEADER
            || d[0] != b'M'
            || d[1] != b'F'
            || d[2] != b'N'
            || d[3] != b'T'
            || d[4] != 1
            || d[5] != 4
            || u32_at(d, 12) != 0
        {
            return Err("invalid MFNT header");
        }
        let count = u16_at(d, 10) as usize;
        let start = HEADER + ENTRY * count;
        if count == 0 || start > d.len() || u16_at(d, 6) == 0 || u16_at(d, 8) > u16_at(d, 6) {
            return Err("invalid MFNT metrics/table");
        }
        let mut i = 0;
        let mut end = 0;
        while i < count {
            let at = HEADER + ENTRY * i;
            let cp = u32_at(d, at);
            if cp > 0x10ffff
                || (cp >= 0xd800 && cp <= 0xdfff)
                || (i > 0 && cp <= u32_at(d, at - ENTRY))
                || u16_at(d, at + 14) != 0
            {
                return Err("invalid MFNT codepoint order");
            }
            let offset = u32_at(d, at + 4) as usize;
            let bytes = (d[at + 10] as usize).div_ceil(2) * d[at + 11] as usize;
            // Generator writes contiguous rasters. Check before addition on 32-bit targets.
            if offset != end || offset > d.len() - start || bytes > d.len() - start - offset {
                return Err("invalid MFNT bitmap bounds");
            }
            end = offset + bytes;
            i += 1;
        }
        if end != d.len() - start {
            return Err("trailing MFNT bitmap bytes");
        }
        Ok(Self(d))
    }
    pub const fn checked(d: &'a [u8]) -> Self {
        match Self::parse(d) {
            Ok(font) => font,
            Err(_) => panic!("invalid built-in MFNT"),
        }
    }
    pub const fn line_height(self) -> i32 {
        u16_at(self.0, 6) as i32
    }
    pub const fn base_line(self) -> i32 {
        u16_at(self.0, 8) as i32
    }
    pub fn count(self) -> usize {
        u16_at(self.0, 10) as usize
    }
    pub fn bitmaps(self) -> &'a [u8] {
        &self.0[HEADER + ENTRY * self.count()..]
    }
    pub fn glyph(self, index: usize) -> Option<Glyph> {
        if index >= self.count() {
            return None;
        }
        let at = HEADER + ENTRY * index;
        Some(Glyph {
            index: index as u32,
            offset: u32_at(self.0, at + 4),
            adv: u16_at(self.0, at + 8),
            w: self.0[at + 10],
            h: self.0[at + 11],
            ofs_x: self.0[at + 12] as i8,
            ofs_y: self.0[at + 13] as i8,
        })
    }
    pub fn code_point(self, index: usize) -> u32 {
        u32_at(self.0, HEADER + ENTRY * index)
    }
    pub fn find(self, letter: u32) -> Option<Glyph> {
        let (mut lo, mut hi) = (0, self.count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.code_point(mid).cmp(&letter) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => return self.glyph(mid),
            }
        }
        None
    }
    pub fn rows(self, glyph: Glyph) -> &'a [u8] {
        let start = glyph.offset as usize;
        &self.bitmaps()[start..start + (glyph.w as usize).div_ceil(2) * glyph.h as usize]
    }
}
