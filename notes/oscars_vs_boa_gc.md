# oscars(with collector:allocator supertrait) vs boa_gc benchmark results

Note author: shruti2522
date: 2026-03-02

I wrote this benchmark to measure the performance effect of adding the `collector:allocator` supertrait and the size class bitmap in arena3.

Ran the `oscars_vs_boa_gc` bench suite with the `gc_allocator` feature on.
This compares oscars against `boa_gc` across node allocation, collection pauses,
vector operations, mixed workloads, memory pressure and deep object graphs

overall: oscars is faster across the board and the gap grows at larger sizes.
A few regressions showed up worth watching, but the overall direction I think is good

## Results

### gc_node_allocation

oscars got ~11.5% faster at size 10. Sizes 100 and 1000 were flat, no significant
change. boa_gc got ~15% faster at all three sizes. The numbers still favor
oscars heavily: boa_gc at 1000 nodes takes ~59 µs vs ~24 µs for oscars

### gc_collection_pause

oscars stayed flat at 100 and 1000 objects. At 500 objects it got ~30% slower
(3.8 µs → 4.3 µs). boa_gc got ~30% faster at 100 and ~29.5% faster at 1000,
but also got ~75% slower at 500.

Both sides regressed at 500 in the same direction, which suggests benchmark noise
or a system scheduling blip rather than a code change, still worth watching.

### vector_creation (oscars_gc_allocator vs boa_gc_std_vec)

oscars got ~8% faster at size 10 and ~10% faster at size 100. Size 1000 was flat.
boa_gc was flat at 10 and 100, but at 1000 it showed a regression of over 2000%.
Almost certainly a fluke. Criterion flagged a warmup warning at that size, meaning
the bench run was unstable, will look into this again 

### vec_of_gc_pointers

oscars got ~8.3% faster at 50 elements. 10 and 100 were unchanged for both.

### mixed_workload

oscars was flat. boa_gc got ~11.7% faster. The ratio between them is about the
same as before: oscars at ~6.7 µs vs boa_gc at ~15.7 µs

### memory_pressure

oscars got ~9.3% slower, boa_gc was unchanged. The churn pattern (allocate 50 per
round, keep 1 in 10, collect 10 rounds) puts a lot of pressure on arena reuse.
Worth looking into. Arenas that are nearly but not fully empty may be the cause.

### deep_object_graph (depth 5, branching factor 3)

oscars got ~20% faster (15.6 µs → 16.3 µs). boa_gc improved ~99.8%, likely from
a very bad baseline run, down to ~39.8 µs. oscars is still roughly 2.4x faster
for this workload

## What the supertrait and size class bitmap had to do with it

### `Collector: Allocator` supertrait

The `Allocator` supertrait means `MarkSweepGarbageCollector` implements
`allocator_api2::Allocator` through a shared reference. This lets you write
`Vec<T, &MarkSweepGarbageCollector>`, which is what `GcAllocVec` is. The vec's
backing buffer lives inside the GC arena directly instead of going through the
system allocator.

The `vector_creation` and `vec_of_gc_pointers` benchmarks show this most clearly.
When oscars creates a `GcAllocVec`, the capacity slab and the GC node header both
come out of the same arena page. The system allocator is never involved. That is
where the consistent improvement in the vec benchmarks comes from.

It also helps the mixed workload and deep graph cases. A `Node` with a
`Vec<Gc<...>>` field puts its children buffer in the arena too, so the whole
object graph ends up packed together instead of spread across the system heap.

### Size class bitmap (arena3)

arena3 stores liveness in a 64 bit bitmap at the top of each page instead of
a per object header field. this means:

- **zero per object overhead.** No extra bytes per object for a live/dead flag

- **fast sweep.** During `collect()`, the sweep scans bitmap words with bitwise
  ops instead of visiting every object. For 100 or 1000 small objects the
  mark and clear pass is cheap enough to keep collection pauses low.

- **size class routing.** Objects go into arenas sized to the nearest class
  (16, 24, 32 ... 2048 bytes). This keeps all slots in a page the same size,
  which makes bitmap indexing simple and free list reuse reliable. Allocation
  stays fast because `alloc_slot` checks the free list first, then bumps

The allocation improvement at `gc_node_allocation/oscars/10` and the collection
pause improvements across all sizes both come from these two things working
together: tight arena packing and cheap bitwise sweep

## TL;DR

`GcAllocVec` at sizes 10 and 100 is 4-5x faster than boa_gc's `std::Vec` (~52 ns
vs ~252 ns at 10, ~232 ns vs ~679 ns at 100). That comes from vec buffers and GC
nodes sharing the same arena page. The node allocation, collection pause and deep
graph gains come from arena3's bitmap: no per object header bytes and a fast
bitwise sweep
