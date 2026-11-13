//! Bounded read-through cache for short immutable physical-page slices.
//! Dhara's journal metadata records are 132 bytes. This is only a cache
//! admission limit; larger reads still go to NAND with their original size.
//! Page/block mutations invalidate entries before touching the medium.
use super::PAGES_PER_BLOCK;

const ENTRIES: usize = 32;
const MAX_BYTES: usize = 132;

#[derive(Clone, Copy)]
struct Entry {
    page: u32,
    offset: usize,
    len: usize,
    data: [u8; MAX_BYTES],
}
impl Entry {
    const EMPTY: Self = Self {
        page: 0,
        offset: 0,
        len: 0,
        data: [0; MAX_BYTES],
    };
}
pub(super) struct ReadCache {
    entries: [Entry; ENTRIES],
    next: usize,
}
impl ReadCache {
    pub const fn new() -> Self {
        Self {
            entries: [Entry::EMPTY; ENTRIES],
            next: 0,
        }
    }
    pub fn clear(&mut self) {
        for entry in &mut self.entries {
            entry.len = 0;
        }
        self.next = 0;
    }
    pub fn read(&self, page: u32, offset: usize, data: &mut [u8]) -> bool {
        if data.is_empty() || data.len() > MAX_BYTES {
            return false;
        }
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.len == data.len() && entry.page == page && entry.offset == offset)
        {
            data.copy_from_slice(&entry.data[..entry.len]);
            true
        } else {
            false
        }
    }
    pub fn insert(&mut self, page: u32, offset: usize, data: &[u8]) {
        if data.is_empty() || data.len() > MAX_BYTES {
            return;
        }
        let entry = &mut self.entries[self.next];
        entry.page = page;
        entry.offset = offset;
        entry.len = data.len();
        entry.data[..data.len()].copy_from_slice(data);
        self.next = (self.next + 1) % ENTRIES;
    }
    pub fn invalidate_page(&mut self, page: u32) {
        for entry in &mut self.entries {
            if entry.page == page {
                entry.len = 0;
            }
        }
    }
    pub fn invalidate_block(&mut self, block: u32) {
        for entry in &mut self.entries {
            if entry.page / PAGES_PER_BLOCK == block {
                entry.len = 0;
            }
        }
    }
}
