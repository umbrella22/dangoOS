//! Exercise the same block walker used by the device allocator.
#[path = "../../../vendor/esp-alloc/src/heap/tlsf.rs"]
mod tlsf;

use core::alloc::Layout;
use tlsf::TlsfHeap;

#[test]
fn free_holes_are_not_reported_as_one_contiguous_block() {
    let mut pool = vec![0u64; 16 * 1024];
    let mut heap = unsafe { TlsfHeap::new(pool.as_mut_ptr().cast(), pool.len() * 8) };
    let initial = heap.block_stats();
    let layout = Layout::from_size_align(16 * 1024, 128).unwrap();
    let mut blocks = Vec::new();
    while let Some(block) = heap.allocate(layout) {
        blocks.push(block);
    }
    assert!(blocks.len() >= 6);
    for (index, &block) in blocks.iter().enumerate() {
        if index % 2 == 0 {
            assert!(unsafe { heap.try_deallocate(block, layout) });
        }
    }
    let stats = heap.block_stats();
    assert_eq!(stats.free, heap.free());
    assert_eq!(stats.used, heap.used());
    assert!(stats.free > stats.largest * 2);
    assert!(stats.free_blocks >= 3);
    assert!(stats.occupied_blocks >= 3);
    let request = Layout::from_size_align(32 * 1024, 128).unwrap();
    assert!(heap.free() > request.size());
    assert!(heap.allocate(request).is_none());
    for (index, &block) in blocks.iter().enumerate() {
        if index % 2 != 0 {
            assert!(unsafe { heap.try_deallocate(block, layout) });
        }
    }
    assert_eq!(heap.block_stats(), initial);
    let block = heap.allocate(request).unwrap();
    assert_eq!(block.as_ptr() as usize % request.align(), 0);
    assert!(unsafe { heap.try_deallocate(block, request) });
    assert_eq!(heap.block_stats(), initial);
    assert_eq!(heap.used(), 0);
}

#[test]
fn resize_reuses_adjacent_space_and_coalesces_after_release() {
    let mut pool = vec![0u64; 16 * 1024];
    let mut heap = unsafe { TlsfHeap::new(pool.as_mut_ptr().cast(), pool.len() * 8) };
    let initial = heap.block_stats();
    let layout = Layout::from_size_align(4096, 256).unwrap();
    let first = heap.allocate(layout).unwrap();
    let adjacent = heap.allocate(layout).unwrap();
    let guard = heap.allocate(layout).unwrap();
    assert!(heap.contains(first));
    unsafe {
        first.as_ptr().write_bytes(0xa5, layout.size());
    }
    assert!(unsafe { heap.try_deallocate(adjacent, layout) });
    let grown = Layout::from_size_align(6144, 256).unwrap();
    assert_eq!(unsafe { heap.reallocate(first, grown) }.unwrap(), first);
    assert_eq!(
        unsafe { core::slice::from_raw_parts(first.as_ptr(), layout.size()) },
        &[0xa5; 4096]
    );
    let shrunk = Layout::from_size_align(512, 256).unwrap();
    assert_eq!(unsafe { heap.reallocate(first, shrunk) }.unwrap(), first);
    assert_eq!(first.as_ptr() as usize % 256, 0);
    assert!(unsafe { heap.try_deallocate(first, shrunk) });
    assert!(unsafe { heap.try_deallocate(guard, layout) });
    assert_eq!(heap.block_stats(), initial);
}

#[test]
fn moving_resize_preserves_bytes_and_failed_resize_preserves_the_old_block() {
    let mut pool = vec![0u64; 16 * 1024];
    let mut heap = unsafe { TlsfHeap::new(pool.as_mut_ptr().cast(), pool.len() * 8) };
    let initial = heap.block_stats();
    let layout = Layout::from_size_align(4096, 128).unwrap();
    let first = heap.allocate(layout).unwrap();
    let guard = heap.allocate(layout).unwrap();
    unsafe {
        first.as_ptr().write_bytes(0x5a, layout.size());
    }
    let larger = Layout::from_size_align(64 * 1024, 128).unwrap();
    let moved = unsafe { heap.reallocate(first, larger) }.unwrap();
    assert_ne!(moved, first);
    assert_eq!(moved.as_ptr() as usize % 128, 0);
    assert_eq!(
        unsafe { core::slice::from_raw_parts(moved.as_ptr(), layout.size()) },
        &[0x5a; 4096]
    );
    let before_failure = heap.block_stats();
    assert!(unsafe {
        heap.reallocate(
            moved,
            Layout::from_size_align(pool.len() * 16, 128).unwrap(),
        )
    }
    .is_none());
    assert_eq!(heap.block_stats(), before_failure);
    assert_eq!(
        unsafe { core::slice::from_raw_parts(moved.as_ptr(), layout.size()) },
        &[0x5a; 4096]
    );
    assert!(unsafe { heap.try_deallocate(moved, larger) });
    assert!(unsafe { heap.try_deallocate(guard, layout) });
    assert_eq!(heap.block_stats(), initial);
}
