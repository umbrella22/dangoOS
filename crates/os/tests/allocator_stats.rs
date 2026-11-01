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
