# Local allocator diagnostics

Base: `esp-rs/esp-hal@33f9b0f7c36ac47edbd015808b5399090fc1f9b6`,
`esp-alloc` 0.11.0. Original file hashes are recorded in `UPSTREAM.json`.
Dependencies keep the same HAL revision. The allocator algorithms, pool layout,
allocation order and C allocation ABI retain their upstream implementations.

`RegionStats` exposes optional TLSF largest-free-payload and block counts via
the existing `rlsf::Tlsf::iter_blocks` API. Sampling holds the existing allocator
lock and costs O(number of blocks); allocation and deallocation gain no walks,
hooks or tracking allocations. The linked-list backend reports these fields as
unavailable. Historical minimum free bytes are not collected.
TLSF snapshots calculate used/free bytes and block metrics in one walk per
region; `HeapStats` reuses that result instead of requesting it a second time.

The largest payload describes a physical free block at alignment 1. TLSF size
classes and larger alignment can reduce a successful request's size. An actual
allocation probe with the consumer's Layout remains necessary.

`EspHeap::probe_caps` performs an allocation and immediate return under the
allocator lock, so concurrent tasks never observe a temporarily exhausted heap.
Probe storage never escapes. The result covers that instant and Layout only;
the free-list order can change. Probes do not count toward allocation history.

Host tests include the production TLSF backend directly and exercise separated
free holes, coalescing, failed large allocations and aligned allocation/free.
