use core::alloc::Layout;
/// Fallible allocation callbacks supplied by the embedding.
pub struct RawAllocator {
    pub alloc: unsafe fn(Layout) -> *mut u8,
    pub dealloc: unsafe fn(*mut u8, Layout),
    /// Resize a live allocation with unchanged alignment. On failure the old
    /// allocation remains valid. The callback may allocate before releasing it.
    pub realloc: Option<unsafe fn(*mut u8, Layout, usize) -> *mut u8>,
}

unsafe fn global_alloc(layout: Layout) -> *mut u8 {
    alloc::alloc::alloc(layout)
}
unsafe fn global_dealloc(ptr: *mut u8, layout: Layout) {
    alloc::alloc::dealloc(ptr, layout)
}
unsafe fn global_realloc(ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
    alloc::alloc::realloc(ptr, layout, size)
}

/// The Rust global allocator.
pub static GLOBAL_ALLOCATOR: RawAllocator = RawAllocator {
    alloc: global_alloc,
    dealloc: global_dealloc,
    realloc: Some(global_realloc),
};
