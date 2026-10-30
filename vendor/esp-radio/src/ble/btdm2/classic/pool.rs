//! In-place allocator for the controller's 3 KiB private pool. Metadata and
//! allocations must remain inside that range for orca's contains_mem check.
//! The caller serializes access and destroys this owner before freeing backing.
use core::{alloc::Layout, ffi::c_void, mem, ptr::{self, NonNull}};
use linked_list_allocator::Heap;

const ALIGN: usize = 8;
const PREFIX: usize = ALIGN;
const HEADER: usize = (mem::size_of::<Heap>() + ALIGN - 1) & !(ALIGN - 1);

pub(super) unsafe fn create(memory: *mut c_void, bytes: usize) -> *mut c_void {
    if memory.is_null()
        || memory as usize % mem::align_of::<Heap>() != 0
        || bytes < HEADER + 32
        || (memory as usize).checked_add(bytes).is_none()
    {
        return ptr::null_mut();
    }
    unsafe {
        let heap = memory.cast::<Heap>();
        heap.write(Heap::empty());
        (*heap).init(memory.cast::<u8>().add(HEADER), bytes - HEADER);
    }
    memory
}

pub(super) unsafe fn destroy(memory: *mut c_void) {
    if !memory.is_null() {
        unsafe { ptr::drop_in_place(memory.cast::<Heap>()) };
    }
}

pub(super) unsafe fn malloc(memory: *mut c_void, bytes: usize) -> *mut c_void {
    if memory.is_null() {
        return ptr::null_mut();
    }
    let Some(total) = bytes.checked_add(PREFIX) else { return ptr::null_mut() };
    let Ok(layout) = Layout::from_size_align(total, ALIGN) else { return ptr::null_mut() };
    unsafe {
        let Ok(block) = (*memory.cast::<Heap>()).allocate_first_fit(layout) else {
            return ptr::null_mut();
        };
        block.cast::<usize>().as_ptr().write(total);
        block.as_ptr().add(PREFIX).cast()
    }
}

pub(super) unsafe fn free(memory: *mut c_void, block: *mut c_void) {
    if block.is_null() { return; }
    // The C allocator contract requires a live block from this pool, once only.
    unsafe {
        let raw = block.cast::<u8>().sub(PREFIX);
        let total = raw.cast::<usize>().read();
        (*memory.cast::<Heap>()).deallocate(
            NonNull::new_unchecked(raw), Layout::from_size_align_unchecked(total, ALIGN));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[repr(align(8))]
    struct Backing([u8; 3072]);

    #[test]
    fn controller_sized_pool_stays_in_range_and_coalesces() {
        let mut backing = Box::new(Backing([0; 3072]));
        let start = backing.0.as_mut_ptr();
        unsafe {
            let pool = create(start.cast(), 3072);
            assert!(!pool.is_null());
            let mut blocks = Vec::new();
            for size in [1, 31, 127, 512, 333, 672] {
                let block = malloc(pool, size).cast::<u8>();
                assert!(!block.is_null());
                assert_eq!(block as usize % ALIGN, 0);
                assert!(block >= start.add(HEADER));
                assert!(block.add(size) <= start.add(3072));
                ptr::write_bytes(block, blocks.len() as u8 + 1, size);
                blocks.push((block, size));
            }
            assert!(malloc(pool, 3072).is_null());
            assert!(malloc(pool, usize::MAX).is_null());
            for (i, &(block, size)) in blocks.iter().enumerate() {
                assert!(core::slice::from_raw_parts(block, size).iter().all(|b| *b == i as u8 + 1));
            }
            for i in [2, 0, 4, 1, 5, 3] { free(pool, blocks[i].0.cast()); }
            let large = malloc(pool, 2800);
            assert!(!large.is_null(), "all freed holes must merge");
            free(pool, large);
            free(pool, ptr::null_mut());
            assert_eq!((*pool.cast::<Heap>()).used(), 0);
            destroy(pool);
        }
    }

    #[test]
    fn rejects_invalid_backing_without_touching_it() {
        let mut backing = Box::new(Backing([0xa5; 3072]));
        let start = backing.0.as_mut_ptr();
        unsafe {
            assert!(create(ptr::null_mut(), 3072).is_null());
            assert!(create(start.add(1).cast(), 3071).is_null());
            assert!(create(start.cast(), HEADER + 1).is_null());
            assert!(malloc(ptr::null_mut(), 1).is_null());
        }
        assert!(backing.0.iter().all(|b| *b == 0xa5));
    }
}
