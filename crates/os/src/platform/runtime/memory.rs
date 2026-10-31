//! Heap allocations with stable addresses; no large temporary pixel arrays.

use alloc::alloc::handle_alloc_error;
use core::{alloc::Layout, ptr::NonNull};

pub struct PixelMemory {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl PixelMemory {
    pub fn new(bytes: usize, external: bool) -> Self {
        let layout = Layout::from_size_align(bytes, 128).expect("pixel allocation layout");
        #[cfg(target_os = "none")]
        let ptr = unsafe {
            let capability = if external {
                esp_alloc::MemoryCapability::External
            } else {
                esp_alloc::MemoryCapability::Internal
            };
            esp_alloc::HEAP.alloc_caps(capability.into(), layout)
        };
        #[cfg(not(target_os = "none"))]
        let ptr = {
            let _ = external;
            unsafe { alloc::alloc::alloc(layout) }
        };
        let ptr = NonNull::new(ptr).unwrap_or_else(|| handle_alloc_error(layout));
        // All bytes, including possible LVGL row padding, are initialized before
        // a flush callback constructs a byte slice over the render buffer.
        unsafe { ptr.as_ptr().write_bytes(0, bytes) };
        Self { ptr, layout }
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }
    pub fn len(&self) -> usize {
        self.layout.size()
    }
}

impl Drop for PixelMemory {
    fn drop(&mut self) {
        #[cfg(target_os = "none")]
        unsafe {
            core::alloc::GlobalAlloc::dealloc(&esp_alloc::HEAP, self.ptr.as_ptr(), self.layout);
        }
        #[cfg(not(target_os = "none"))]
        unsafe {
            alloc::alloc::dealloc(self.ptr.as_ptr(), self.layout)
        };
    }
}
