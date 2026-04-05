# GC API Redesign Proposal

**Status**: RFC

## Problem

Current `boa_gc` uses implicit rooting via `Clone`/`Drop` on `Gc<T>`. Every clone touches root counts, adding overhead in hot VM paths. It also needs `thread_local`, blocking `no_std`.

This proposes lifetime-branded `Gc<'gc, T>` for zero cost pointers and explicit `Root<'id, T>` for persistence.

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
pub struct WeakGc<'id, T: Trace + ?Sized> {
    ptr: NonNull<GcBox<T>>,
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace + ?Sized> WeakGc<'id, T> {
    pub fn upgrade<'gc>(&self, cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, T>> { ... }
}
```
Weak references carry the same `'id` brand as the context they came from. `upgrade` requires a matching `MutationContext<'id, 'gc>`, so cross-context upgrade is a compile error.

The `'gc` lifetime ties the pointer to its collector. Copying is free, no root count manipulation.

### Root for Persistence

```rust
pub struct Root<'id, T: Trace> {
    raw: NonNull<RootNode<'id, T>>,
}

#[repr(C)]
pub(crate) struct RootNode<'id, T: Trace> {
    link: RootLink,        // at offset 0, bare link* == RootNode*
    gc_ptr: NonNull<GcBox<T>>, // T: Sized keeps this thin for type-erased offset_of!
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace> Root<'id, T> {
    pub fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Gc<'gc, T> { ... }
}

impl<'id, T: Trace> Drop for Root<'id, T> {
    fn drop(&mut self) {
        unsafe {
            let node = Box::from_raw(self.raw.as_ptr());
            if node.link.is_linked() {
                RootLink::unlink(NonNull::from(&node.link));
            }
        }
    }
}
```

`Root<'id, T>` escapes the `'gc` lifetime but is tied to the `GcContext<'id>` that created it. The node is heap-allocated via `Box::into_raw`, keeping its address stable for the intrusive list without requiring `Pin` on the public API. `Drop` reclaims the allocation after unlinking. Cross-context misuse is a compile error, not a runtime panic.

**No `Rc` required.** A root only needs its own embedded `prev`/`next` pointers to remove itself from the list. The `Collector` owns a **sentinel** node; insertion and removal are pure pointer surgery with no allocation and no reference counting.

### MutationContext

```rust
pub struct MutationContext<'id, 'gc> {
    collector: &'gc Collector,
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id, 'gc> MutationContext<'id, 'gc> {
    pub fn alloc<T: Trace>(&self, value: T) -> Gc<'gc, T> { ... }
    pub fn alloc_weak<T: Trace>(&self, value: T) -> WeakGc<'id, T> { ... }
    pub fn root<T: Trace>(&self, gc: Gc<'gc, T>) -> Root<'id, T> { ... }
    pub fn collect(&self) { ... }
}
```

Uses `&self` with `RefCell` inside for multiple concurrent allocations.

### Sentinel Node & Root Traversal

The `Collector` owns one **pinned sentinel** `RootLink` (a bare link node with no payload):

```text
Collector::sentinel -> root_a.link -> root_b.link -> root_c.link -> None
```

Roots insert themselves immediately after the sentinel via `RootLink::link_after`. During collection, `RootLink::iter_from_sentinel(sentinel)` starts from `sentinel.next`, so the sentinel itself is never yielded. For each link, `gc_ptr` is recovered via `offset_of!(RootNode<i32>, gc_ptr)` and used to mark the allocation. A `debug_assert_eq!` with a second concrete type verifies the offset is stable across all `T: Sized`.

### Entry Point

```rust
pub struct GcContext<'id> {
    collector: Collector,
    _marker: PhantomData<*mut &'id ()>,
}

pub fn with_gc<R, F: for<'id> FnOnce(GcContext<'id>) -> R>(f: F) -> R { ... }

impl<'id> GcContext<'id> {
    pub fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'id, 'gc>) -> R) -> R { ... }
}
```

`with_gc` is the only way to create a `GcContext`. The `for<'id>` bound gives each context a fresh, unique lifetime that cannot unify with any other context's `'id`. `GcContext::mutate` threads that same `'id` into every `MutationContext` produced inside the closure.

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
| **Isolation** | Runtime only | Compile-time only |

## Why This Works

**no_std Compatible**: No `thread_local` needed.

**Performance**: `Gc` copying is just memcpy, no root count overhead.

**Allocation**: Uses `mempool3::PoolAllocator` with size-class pooling instead of individual `Box` allocations, avoiding fragmentation.

**Safety**:
- Cross-context use of `Gc`, `Root`, and `WeakGc` is a compile error, not a runtime panic
- No `collector_id` field, no atomic counter, no branch in `Root::get`
- Explicit `!Send`/`!Sync` prevents threading bugs
- Intrusive sentinel-based linked list for O(1) insertion and self-unlink
- `Root` holds **no `Rc`**, unlink is pure pointer surgery on embedded `prev`/`next`
- Node address stability comes from `Box::into_raw`, `Pin` is not required on the public API

## Open Questions

- FFI boundaries (native functions receiving `Gc` pointers)
- Migration path (thousands of `Gc<T>` uses in Boa)
- Real benchmark numbers

## References

- gc-arena: https://github.com/kyren/gc-arena
- boa#2631: https://github.com/boa-dev/boa/issues/2631
