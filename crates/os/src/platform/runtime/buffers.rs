//! Service buffers with explicit External memory capability on device.
use core::{alloc::Layout, ptr::NonNull};
/// A fallible owned byte allocation for service buffers. Device allocations
/// require External capability, so a fragmented PSRAM heap cannot consume the
/// small Internal pool reserved for DMA, radio and executor stacks.
pub struct ExternalBytes {
    ptr: NonNull<u8>,
    layout: Layout,
}
impl ExternalBytes {
    pub fn try_zeroed(bytes: usize) -> Result<Self, &'static str> {
        if bytes == 0 {
            return Err("empty external allocation");
        }
        let layout = Layout::from_size_align(bytes, 4).map_err(|_| "external allocation layout")?;
        #[cfg(target_os = "none")]
        let p = unsafe {
            esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout)
        };
        #[cfg(not(target_os = "none"))]
        let p = unsafe { alloc::alloc::alloc(layout) };
        let ptr = NonNull::new(p).ok_or("contiguous external memory unavailable")?;
        // SAFETY: exclusive new allocation of exactly bytes, initialized before
        // constructing any safe view; the same allocator frees it in Drop.
        unsafe { ptr.as_ptr().write_bytes(0, bytes) };
        Ok(Self { ptr, layout })
    }
}
impl core::ops::Deref for ExternalBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: this owner retains initialized storage for the borrow.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.layout.size()) }
    }
}
impl core::ops::DerefMut for ExternalBytes {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: exclusive owner borrow, no retained aliases.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.layout.size()) }
    }
}
// SAFETY: sole owned allocation; it has no device handles or thread affinity.
unsafe impl Send for ExternalBytes {}
impl Drop for ExternalBytes {
    fn drop(&mut self) {
        #[cfg(target_os = "none")]
        unsafe {
            core::alloc::GlobalAlloc::dealloc(&esp_alloc::HEAP, self.ptr.as_ptr(), self.layout)
        };
        #[cfg(not(target_os = "none"))]
        unsafe {
            alloc::alloc::dealloc(self.ptr.as_ptr(), self.layout)
        };
    }
}
