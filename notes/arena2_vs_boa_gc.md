# arena2 vs boa_gc benchmark results

Note author: shruti2522
date: 2026-03-06

This benchmark measures how the `arena2` allocator which uses a simple bump allocator with `TaggedPtr` headers for liveness compares against the standard `boa_gc` implementation

Ran the `arena2_vs_boa_gc` bench suite. It compares oscars' `arena2` against `boa_gc` across node allocation, collection pauses, mixed workloads, and memory pressure.

## Results

### gc_node_allocation

arena2 heavily outperforms boa_gc across all sizes.
- **10 nodes:** arena2 takes ~320 ns vs ~750 ns for boa_gc
- **100 nodes:** arena2 takes ~3.2 µs vs ~6.4 µs for boa_gc
- **1000 nodes:** arena2 takes ~27.3 µs vs ~56.2 µs  for boa_gc

This shows that bump allocation into an arena page is consistently more than 2x faster than whatever the standard boa_gc is doing.

### gc_collection_pause

Similar to allocations, the sweep phase in arena2 is extremely fast compared to boa_gc.
- **100 objects:** arena2 sweeps in ~3.5 µs vs ~7.3 µs for boa_gc
- **500 objects:** arena2 sweeps in ~15.2 µs vs ~32.5 µs for boa_gc
- **1000 objects:** arena2 sweeps in ~29.5 µs vs ~74.9 µs for boa_gc

The linear scan over the contiguous blocks in arena2 during garbage collection cuts the pause times by more than half.

### mixed_workload

This tests repeated allocations spread around `collect()` pauses.
Both allocators performed similarly here. arena2 took ~17.8 µs and boa_gc took ~17.8 µs. So arena2's big speed advantage seems to even out when allocations and collections are mixed together.

### memory_pressure

This tests creating and deleting many objects quickly (make 50, keep 5, collect, repeat 10 times).
both allocators are equally fast here. arena2 took ~46.0 µs and boa_gc took ~46.6 µs. The cost of throwing away whole memory pages versus single objects seems to balance out

## Conclusion

`arena2` is much faster for simple allocations and collection sweeps, about twice as fast. In mixed tests and heavy memory tests, they perform about the same.
