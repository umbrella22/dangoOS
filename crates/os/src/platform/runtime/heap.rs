//! Capability-specific heap snapshots. Missing measurements remain explicit.
use lua_engine::{caps, heap_fields, HeapInfo};

#[derive(Clone, Copy)]
struct Region {
    external: bool,
    size: u32,
    used: u32,
    free: u32,
    largest: Option<u32>,
    allocated_blocks: Option<u32>,
    free_blocks: Option<u32>,
}

fn summarize(regions: impl Iterator<Item = Region>, requested: u32) -> HeapInfo {
    if requested & !(caps::DEFAULT | caps::INTERNAL | caps::SPIRAM) != 0 {
        return HeapInfo::default();
    }
    let mut info = HeapInfo {
        valid_fields: heap_fields::SIZES | heap_fields::LARGEST_FREE | heap_fields::BLOCK_COUNTS,
        ..HeapInfo::default()
    };
    for r in regions.filter(|r| {
        (requested & caps::SPIRAM == 0 || r.external)
            && (requested & caps::INTERNAL == 0 || !r.external)
    }) {
        info.total_size += r.size;
        info.free_size += r.free;
        info.allocated_size += r.used;
        if let Some(largest) = r.largest {
            info.largest_free_block = info.largest_free_block.max(largest);
        } else {
            info.valid_fields &= !heap_fields::LARGEST_FREE;
        }
        if let (Some(allocated), Some(free)) = (r.allocated_blocks, r.free_blocks) {
            info.allocated_blocks += allocated;
            info.free_blocks += free;
            info.total_blocks += allocated + free;
        } else {
            info.valid_fields &= !heap_fields::BLOCK_COUNTS;
        }
    }
    info
}

#[cfg(target_os = "none")]
pub fn snapshot(requested: u32) -> HeapInfo {
    use core::sync::atomic::{AtomicU32, Ordering};
    static SAMPLED_MIN: [AtomicU32; 3] = [const { AtomicU32::new(u32::MAX) }; 3];
    let stats = esp_alloc::HEAP.stats();
    let mut info = summarize(
        stats.region_stats.iter().flatten().map(|r| Region {
            external: r
                .capabilities
                .contains(esp_alloc::MemoryCapability::External),
            size: r.size as u32,
            used: r.used as u32,
            free: r.free as u32,
            largest: r.largest_free_block.map(|v| v as u32),
            allocated_blocks: r.allocated_blocks.map(|v| v as u32),
            free_blocks: r.free_blocks.map(|v| v as u32),
        }),
        requested,
    );
    let index = match requested & (caps::INTERNAL | caps::SPIRAM) {
        0 => Some(0),
        caps::INTERNAL => Some(1),
        caps::SPIRAM => Some(2),
        _ => None,
    };
    if info.valid_fields & heap_fields::SIZES != 0 {
        if let Some(index) = index {
            info.sampled_minimum_free_size = SAMPLED_MIN[index]
                .fetch_min(info.free_size, Ordering::Relaxed)
                .min(info.free_size);
            info.valid_fields |= heap_fields::SAMPLED_MINIMUM_FREE;
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    fn region(external: bool) -> Region {
        Region {
            external,
            size: 1000,
            used: 400,
            free: 600,
            largest: Some(200),
            allocated_blocks: Some(4),
            free_blocks: Some(3),
        }
    }
    #[test]
    fn aggregation_preserves_contiguity_and_intersects_capabilities() {
        let regions = [region(true), region(false), region(false)];
        let i = summarize(regions.into_iter(), caps::DEFAULT);
        assert_eq!(
            (i.free_size, i.largest_free_block, i.total_blocks),
            (1800, 200, 21)
        );
        assert_eq!(i.valid_fields & heap_fields::MINIMUM_FREE, 0);
        assert_eq!(
            summarize(regions.into_iter(), caps::INTERNAL).free_size,
            1200
        );
        assert_eq!(summarize(regions.into_iter(), caps::SPIRAM).free_size, 600);
        assert_eq!(
            summarize(regions.into_iter(), caps::SPIRAM | caps::INTERNAL).free_size,
            0
        );
        assert_eq!(summarize(regions.into_iter(), 1).valid_fields, 0);
    }
    #[test]
    fn missing_metrics_are_not_fabricated_from_other_regions() {
        let mut unknown = region(true);
        unknown.largest = None;
        unknown.free_blocks = None;
        let i = summarize([region(false), unknown].into_iter(), caps::DEFAULT);
        assert_eq!(i.free_size, 1200);
        assert_eq!(i.valid_fields, heap_fields::SIZES);
    }
}
