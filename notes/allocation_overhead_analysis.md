# Allocation overhead analysis

Note author: shruti2522

This note investigates allocation overhead in oscars allocators, particularly the
double header issue where allocator boxes and GC boxes both carry metadata.

## Current structure

arena2 allocates traced objects as `ArenaHeapItem<GcBox<T>>`:

```rust
pub struct ArenaHeapItem<T: ?Sized> {
    next: TaggedPtr<ErasedHeapItem>,  // linked list + dropped flag
    value: T,
}

pub struct GcBox<T: Trace + ?Sized> {
    header: GcHeader,        // flags + root_count
    vtable: &'static VTable, // trace/drop/finalize function pointers
    value: T,
}
```

Both allocator and GC layers add metadata overhead before the actual value.

**mempool3 comparison:** Uses `repr(transparent)` PoolItem wrapper with zero per-object
overhead. Liveness tracked in per-page bitmaps instead of per-object headers. Achieves
this via bitmap tracking and fixed-size slots instead of linked lists.

## Why both headers exist

The allocator and GC track different states:

- **Allocator:** walks `next` linked list to verify all allocations were dropped before
  arena reset
- **GC:** uses `root_count` and color bits to track reachability and marking state

An object can be allocated with `root_count == 0` (unreachable garbage). The GC knows
it is collectible, but the allocator still needs the linked list for cleanup verification.
These are separate concerns that cannot be trivially merged.

## Alternative approaches

**Page-level bitmaps:**
Replace per-object `next` pointers with per-page bitmaps like mempool3. Requires
redesigning arena2 into a paged allocator with bitmap scanning during cleanup.

**Size-class pools:**
Use bitmaps for small objects, keep linked lists for larger objects. Cuts overhead
where it matters most but adds implementation complexity.

**Metadata co-location:**
Store allocator bits in GcBox header padding. Does not reduce total overhead, just
relocates it. Creates coupling between GC and allocator layouts.

## GC policy impact

Different collectors have different requirements:

- **Mark-sweep:** needs to walk all allocated objects during sweep. arena2's linked list
  supports this. Eliminating it requires page scanning.
- **Mark-compact / Generational:** could share header space for compaction metadata or
  generation tracking.
- **Copying:** could reuse allocator header for forwarding pointers during collection.

Current separation makes experimenting with different GC policies easier without
redesigning the allocator.

## Conclusion

arena2's overhead from both allocator and GC headers serves two distinct purposes that
cannot be trivially merged. Potential reductions require significant architectural changes:

- **Page-level bitmaps:** best overhead reduction but requires paged allocator redesign
- **Size-class pools:** practical middle ground, optimizes common case
- **Metadata co-location:** no overhead savings, just reorganization

All need profiling on real Boa workloads to justify complexity. Current design prioritizes
allocation speed, type flexibility and allocator independence. Future work should
profile actual allocation patterns to determine if overhead matters and which size
classes dominate.
