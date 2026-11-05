use core::{alloc::Layout, ptr::NonNull};

use rlsf::Tlsf;

// TODO: make this configurable
type Heap = Tlsf<'static, usize, usize, { usize::BITS as usize }, { usize::BITS as usize }>;

pub(crate) struct TlsfHeap {
    heap: Heap,
    pool_start: usize,
    pool_end: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BlockStats {
    pub used: usize,
    pub free: usize,
    pub largest: usize,
    pub occupied_blocks: usize,
    pub free_blocks: usize,
}

impl TlsfHeap {
    /// One walk under the allocator lock. The largest payload is for alignment 1;
    /// callers must probe their actual Layout separately.
    pub fn block_stats(&self) -> BlockStats {
        let mut stats = BlockStats {
            used: 0,
            free: 0,
            largest: 0,
            occupied_blocks: 0,
            free_blocks: 0,
        };
        let pool =
            unsafe { core::slice::from_raw_parts(self.pool_start as *const u8, self.size()) };
        for block in unsafe { self.heap.iter_blocks(NonNull::from(pool)) } {
            if block.is_occupied() {
                stats.used += block.size();
                stats.occupied_blocks += 1;
            } else {
                stats.free += block.max_payload_size();
                stats.free_blocks += 1;
                stats.largest = stats.largest.max(block.max_payload_size());
            }
        }
        stats
    }

    pub unsafe fn new(heap_bottom: *mut u8, size: usize) -> Self {
        let mut heap = Heap::new();

        let block = unsafe { core::slice::from_raw_parts(heap_bottom, size) };
        let actual_size = unsafe { heap.insert_free_block_ptr(block.into()).unwrap() };

        Self {
            heap,
            pool_start: heap_bottom as usize,
            pool_end: heap_bottom as usize + actual_size.get(),
        }
    }

    pub fn size(&self) -> usize {
        self.pool_end - self.pool_start
    }

    pub fn used(&self) -> usize {
        let mut used = 0;
        let pool =
            unsafe { core::slice::from_raw_parts(self.pool_start as *const u8, self.size()) };
        for block in unsafe { self.heap.iter_blocks(NonNull::from(pool)) } {
            if block.is_occupied() {
                used += block.size();
            }
        }
        used
    }

    pub fn free(&self) -> usize {
        let mut free = 0;
        let pool =
            unsafe { core::slice::from_raw_parts(self.pool_start as *const u8, self.size()) };
        for block in unsafe { self.heap.iter_blocks(NonNull::from(pool)) } {
            if !block.is_occupied() {
                free += block.max_payload_size();
            }
        }
        free
    }

    pub fn allocate(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        self.heap.allocate(layout)
    }

    pub(crate) fn contains(&self, ptr: NonNull<u8>) -> bool {
        let address = ptr.addr().get();
        self.pool_start <= address && address < self.pool_end
    }

    /// Resizing retains this pool and the original alignment. Failure leaves
    /// the allocation untouched; TLSF attempts in-place resize before moving.
    pub(crate) unsafe fn reallocate(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
    ) -> Option<NonNull<u8>> {
        unsafe { self.heap.reallocate(ptr, layout) }
    }

    pub(crate) unsafe fn try_deallocate(&mut self, ptr: NonNull<u8>, layout: Layout) -> bool {
        let addr = ptr.addr().get();
        if self.pool_start <= addr && self.pool_end > addr {
            unsafe { self.heap.deallocate(ptr, layout.align()) };
            true
        } else {
            false
        }
    }
}
