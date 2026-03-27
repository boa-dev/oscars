# GC API Redesign Proposal

**Status**: RFC

## Problem

Current `boa_gc` uses implicit rooting via `Clone`/`Drop` on `Gc<T>`. Every clone touches root counts, adding overhead in hot VM paths. It also needs `thread_local`, blocking `no_std`.

This proposes lifetime-branded `Gc<'gc, T>` for zero cost pointers and explicit `Root<T>` for persistence.

## Core API

### Gc Pointer

```rust
pub struct Gc<'gc, T: Trace> {
    ptr: NonNull<GcBox<T>>,
    _marker: PhantomData<&'gc T>,
}

impl<'gc, T: Trace> Copy for Gc<'gc, T> {}
impl<T: Trace + ?Sized> !Send for Gc<'_, T> {}
impl<T: Trace + ?Sized> !Sync for Gc<'_, T> {}
```

The `'gc` lifetime ties the pointer to its collector. Copying is free, no root count manipulation.

### Root for Persistence

```rust
pub struct Root<T: Trace + ?Sized> {
    ptr: NonNull<GcBox<T>>,
    collector_id: u64,
    collector_roots: *const RefCell<Vec<RootEntry>>,
}

impl<T: Trace> Root<T> {
    pub fn get<'gc>(&self, cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        assert_eq!(self.collector_id, cx.collector.id);
        // ...
    }
}

impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        // Unregister from root list
    }
}
```

`Root<T>` escapes the `'gc` lifetime. Stores collector ID to catch cross-collector misuse.

### MutationContext

```rust
pub struct MutationContext<'gc> {
    collector: &'gc Collector,
}

impl<'gc> MutationContext<'gc> {
    pub fn alloc<T: Trace>(&self, value: T) -> Gc<'gc, T> { ... }
    pub fn root<T: Trace>(&self, gc: Gc<'gc, T>) -> Root<T> { ... }
}
```

Uses `&self` with `RefCell` inside for multiple concurrent allocations.

### Entry Point

```rust
pub fn with_gc<R>(f: impl for<'gc> FnOnce(MutationContext<'gc>) -> R) -> R {
    let collector = Collector::new();
    f(MutationContext { collector: &collector })
}
```

The `for<'gc>` pattern from gc-arena creates unique lifetime per arena.

## vs Current Oscars

| | Current | Proposed |
|---|---------|----------|
| **Pointer** | `Gc<T>` | `Gc<'gc, T>` |
| **Lifetime** | `'static` + `extend_lifetime()` | `'gc` branded |
| **Rooting** | Implicit (inc/dec on clone/drop) | Explicit (`Root<T>`) |
| **Copy cost** | Cell write | Zero |
| **Isolation** | Runtime only | Compile-time + runtime validation |

## Why This Works

**no_std Compatible**: No `thread_local` needed.

**Performance**: `Gc` copying is just memcpy, no root count overhead.

**Safety**: 
- Cross-context caught at compile time for `Gc`
- Cross-collector caught at runtime for `Root`
- Explicit `!Send`/`!Sync` prevents threading bugs

## Open Questions

- FFI boundaries (native functions receiving `Gc` pointers)
- Migration path (thousands of `Gc<T>` uses in Boa)
- Real benchmark numbers

## References

- gc-arena: https://github.com/kyren/gc-arena
- boa#2631: https://github.com/boa-dev/boa/issues/2631
