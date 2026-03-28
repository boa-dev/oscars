# GC API Redesign Proposal

**Status**: RFC

## Problem

Current `boa_gc` uses implicit rooting via `Clone`/`Drop` on `Gc<T>`. Every clone touches root counts, adding overhead in hot VM paths. It also needs `thread_local`, blocking `no_std`.

This proposes lifetime-branded `Gc<'gc, T>` for zero cost pointers and explicit `Root<T>` for persistence.

## Core API

### Gc Pointer

```rust
pub struct Gc<'gc, T: Trace + ?Sized + 'gc> {
    ptr: NonNull<GcBox<T>>,
    _marker: PhantomData<(&'gc T, *const ())>,
}

impl<'gc, T: Trace + ?Sized + 'gc> Copy for Gc<'gc, T> {}
```

### Mutability via GcRefCell
```rust
pub struct GcRefCell<T: Trace> {
    inner: RefCell<T>,
}
```
`GcRefCell` safely traces internal values statically behind a dynamically borrowed `RefCell`, providing `GcRef` and `GcRefMut` access similar to native `Rc/RefCell` combinations. Allows internal JavaScript arrays and objects to be mutated during the GC trace safely.

### Weak Reference Separation
```rust
pub struct WeakGc<T: Trace + ?Sized> {
    ptr: NonNull<GcBox<T>>,
}

impl<T: Trace + ?Sized> WeakGc<T> {
    pub fn upgrade<'gc>(&self, cx: &MutationContext<'gc>) -> Option<Gc<'gc, T>> { ... }
}
```
Weak references drop their tie to the single `'gc` lifetime. Instead, they are upgraded back into strong `Gc` pointers only when explicitly bound against an active safe `MutationContext<'gc>`.

The `'gc` lifetime ties the pointer to its collector. Copying is free, no root count manipulation.

### Root for Persistence

```rust
pub struct Root<T: Trace> {
    link: RootLink,  // Intrusive list node (prev/next only), at offset 0 so bare link* == Root*
    gc_ptr: NonNull<GcBox<T>>, // T: Sized keeps this thin for type erased offset_of!
    /// Cross collector misuse detection only, plays no role in unlinking.
    collector_id: u64,
    _marker: PhantomData<*const ()>,
}

impl<T: Trace> Root<T> {
    pub fn get<'gc>(&self, cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        assert_eq!(self.collector_id, cx.collector.id);
        // ...
    }
}

impl<T: Trace> Drop for Root<T> {
    fn drop(&mut self) {
        // O(1) self unlink: splice prev/next together, no list reference needed
        if self.link.is_linked() {
            unsafe {
                RootLink::unlink(NonNull::from(&self.link));
            }
        }
    }
}
```

`Root<T>` escapes the `'gc` lifetime. Returned as `Pin<Box<Root<T>>>` for stable addresses (required by the intrusive list). Stores `collector_id` to catch cross-collector misuse at runtime — it is **not** used during unlink; `Drop` only touches the embedded `prev`/`next` pointers.

**No `Rc` required.** A root only needs its own embedded `prev`/`next` pointers to remove itself from the list. The `Collector` owns a **sentinel** node; insertion and removal are pure pointer surgery with no allocation and no reference counting.

### MutationContext

```rust
pub struct MutationContext<'gc> {
    collector: &'gc Collector,
}

impl<'gc> MutationContext<'gc> {
    pub fn alloc<T: Trace>(&self, value: T) -> Gc<'gc, T> { ... }
    pub fn root<T: Trace>(&self, gc: Gc<'gc, T>) -> Pin<Box<Root<T>>> { ... }
    pub fn collect(&self) { ... }
}
```

Uses `&self` with `RefCell` inside for multiple concurrent allocations.

### Sentinel Node & Root Traversal

The `Collector` owns one **pinned sentinel** `RootLink` (a bare link node with no payload):

```text
Collector::sentinel -> root_a.link -> root_b.link -> root_c.link -> None
```

Roots insert themselves immediately after the sentinel via `RootLink::link_after`. During collection, `RootLink::iter_from_sentinel(sentinel)` starts from `sentinel.next`, so the sentinel itself is never yielded. For each link, `gc_ptr` is recovered via `offset_of!(Root<i32>, gc_ptr)` and used to mark the allocation.

### Entry Point

```rust
pub struct GcContext {
    collector: Collector,
}

impl GcContext {
    pub fn new() -> Self { ... }
    pub fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'gc>) -> R) -> R { ... }
}
```

By owning the `Collector`, `GcContext` defines the entire host timeline. The `for<'gc>` pattern from gc-arena creates a unique lifetime isolating active context mutations per arena.

### Tracing Mechanism
```rust
pub trait Trace {
    fn trace(&mut self, tracer: &mut Tracer);
}

pub trait Finalize {
    fn finalize(&self) {}
}
```
Note: `trace` takes `&mut self` instead of `&self`, ensuring that potential moving collectors have exclusive layout rights during traces.

## vs Current Oscars

| | Current | Proposed |
|---|---------|----------|
| **Pointer** | `Gc<T>` | `Gc<'gc, T>` |
| **Lifetime** | `'static` + `extend_lifetime()` | `'gc` branded |
| **Rooting** | Implicit (inc/dec on clone/drop) | Explicit (`Root<T>`) |
| **Copy cost** | Cell write | Zero |
| **Drop cost** | TLS access (futex lock) | Zero (Copy type) |
| **Isolation** | Runtime only | Compile-time + runtime validation |

## Why This Works

**no_std Compatible**: No `thread_local` needed.

**Performance**: `Gc` copying is just memcpy, no root count overhead.

**Allocation**: Uses `mempool3::PoolAllocator` with size-class pooling instead of individual `Box` allocations, avoiding fragmentation.

**Safety**:
- Cross-context caught at compile time for `Gc`
- Cross-collector caught at runtime for `Root`
- Explicit `!Send`/`!Sync` prevents threading bugs
- Intrusive sentinel based linked list for O(1) insertion and self-unlink
- `Root` holds **no `Rc`**, unlink is pure pointer surgery on embedded `prev`/`next`
- `Pin<Box<Root<T>>>` guarantees stable node addresses while linked

## Open Questions

- FFI boundaries (native functions receiving `Gc` pointers)
- Migration path (thousands of `Gc<T>` uses in Boa)
- Real benchmark numbers

## References

- gc-arena: https://github.com/kyren/gc-arena
- boa#2631: https://github.com/boa-dev/boa/issues/2631
