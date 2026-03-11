# Boa GC API Surface

## 1. Overview

The Boa JavaScript engine depends on the `boa_gc` crate for all garbage-collected memory
management. This document defines the subset of the `boa_gc` public API that the engine
actually uses at compile time and runtime.

Any alternative garbage collector aiming to replace `boa_gc` (for example, Oscars GC)
**must implement this interface** to serve as a drop-in replacement. APIs not listed here
are internal to the collector and are not part of the engine-facing contract.

## Methodology

The API surface documented here was derived by inspecting the current
`boa_gc` usage inside the Boa repository.

The process included:

- Searching for `use boa_gc::` imports across the engine
- Inspecting how core types (`Gc`, `WeakGc`, `GcCell`, `Trace`, `Finalize`)
  are used throughout the codebase
- Reviewing `Trace` and `Finalize` implementations to understand the
  traversal contract
- Inspecting weak structures (`WeakMap`, `WeakRef`) to identify required
  weak reference semantics

This was done through manual inspection of the Boa engine source to
extract the GC interface boundary relied upon by the runtime.

---

## 2. Core Pointer Types

| Type | Role |
|---|---|
| `Gc<T>` | Strong, trace-aware smart pointer. Primary way the engine holds GC-managed values. |
| `WeakGc<T>` | Weak reference that does not prevent collection. Used for caches and JS `WeakRef`. |
| `GcRefCell<T>` | Interior-mutability wrapper for values stored behind a `Gc`. Analogous to `RefCell`. |
| `GcRef<'a, T>` | Immutable borrow guard returned by `GcRefCell::borrow`. |
| `GcRefMut<'a, T>` | Mutable borrow guard returned by `GcRefCell::borrow_mut`. |

These five types appear in virtually every subsystem of the engine: the object model,
environments, bytecode compiler, module system, and builtins.

Example usage in Boa:

- https://github.com/boa-dev/boa/blob/main/core/engine/src/object/jsobject.rs
- https://github.com/boa-dev/boa/blob/main/core/engine/src/value/mod.rs

---

## 3. Pointer Operations

### Allocation

```rust
// Allocate a new GC-managed value.
Gc::new(value: T) -> Gc<T>

// Allocate a value that may reference itself through a weak pointer.
Gc::new_cyclic<F>(data_fn: F) -> Gc<T>
where F: FnOnce(&WeakGc<T>) -> T
```

### Cloning & Identity

```rust
// Duplicate the smart pointer (increments root tracking).
impl Clone for Gc<T>

// Compare two pointers by address, not by value.
Gc::ptr_eq(this: &Gc<T>, other: &Gc<U>) -> bool
```

### Raw Pointer Conversion

Used at FFI boundaries (native function closures, synthetic modules).

```rust
// Consume the Gc and return a raw pointer. Must be paired with from_raw.
Gc::into_raw(this: Gc<T>) -> NonNull<GcBox<T>>

// Reconstruct a Gc from a raw pointer previously obtained via into_raw.
unsafe fn Gc::from_raw(ptr: NonNull<GcBox<T>>) -> Gc<T>
```

### Type Casting

Used by the object model to downcast erased object types.

```rust
// Runtime type check and downcast.
Gc::downcast<U>(this: Gc<T>) -> Option<Gc<U>>

// Unchecked downcast. Caller must guarantee correctness.
unsafe fn Gc::cast_unchecked<U>(this: Gc<T>) -> Gc<U>

// Unchecked reference cast without consuming the pointer.
unsafe fn Gc::cast_ref_unchecked<U>(this: &Gc<T>) -> &Gc<U>
```

### Dereferencing

```rust
impl Deref for Gc<T> { type Target = T; }
```

`Gc<T>` transparently dereferences to `T`, allowing direct field and method access.

---

## 4. Interior Mutability API

### GcRefCell

```rust
GcRefCell::new(value: T) -> GcRefCell<T>

GcRefCell::borrow(&self)         -> GcRef<'_, T>
GcRefCell::borrow_mut(&self)     -> GcRefMut<'_, T>
GcRefCell::try_borrow(&self)     -> Result<GcRef<'_, T>, BorrowError>
GcRefCell::try_borrow_mut(&self) -> Result<GcRefMut<'_, T>, BorrowMutError>
GcRefCell::into_inner(self)      -> T
```

### Borrow Guard Mapping

`GcRef` and `GcRefMut` support projecting the borrow into a sub-field of the
contained value, similar to `std::cell::Ref::map`.

```rust
GcRef::map<U>(orig: GcRef<'_, T>,  f: F) -> GcRef<'_, U>
GcRef::try_map<U>(orig: GcRef<'_, T>, f: F) -> Result<GcRef<'_, U>, GcRef<'_, T>>
GcRef::cast<U>(orig: GcRef<'_, T>) -> GcRef<'_, U>   // unsafe or checked downcast

GcRefMut::map<U>(orig: GcRefMut<'_, T>,  f: F) -> GcRefMut<'_, U>
GcRefMut::try_map<U>(orig: GcRefMut<'_, T>, f: F) -> Result<GcRefMut<'_, U>, GcRefMut<'_, T>>
GcRefMut::cast<U>(orig: GcRefMut<'_, T>) -> GcRefMut<'_, U>
```

---

## 5. Weak Reference System

### WeakGc

```rust
// Create a weak reference from a strong pointer.
WeakGc::new(value: &Gc<T>) -> WeakGc<T>

// Attempt to obtain a strong reference. Returns None if the object was collected.
WeakGc::upgrade(&self) -> Option<Gc<T>>
```

A `WeakGc<T>` **must not** prevent its referent from being collected. After collection,
`upgrade()` must return `None`.

### Ephemeron

An `Ephemeron<K, V>` is a key-value pair where the value is only considered reachable
if the key is independently reachable. `WeakGc<T>` is implemented internally as
`Ephemeron<T, ()>`.

### WeakMap

`WeakMap<K, V>` is a GC-aware associative container whose entries are automatically
removed when their key becomes unreachable. It is used by the engine to implement the
ECMAScript `WeakMap` and `WeakSet` builtins.

---

## 6. GC Traits

### Trace

```rust
pub unsafe trait Trace {
    /// Mark all GC pointers contained within this value.
    unsafe fn trace(&self, tracer: &mut Tracer);

    /// Count non-root references during the root-detection phase.
    unsafe fn trace_non_roots(&self);

    /// Execute the associated finalizer.
    fn run_finalizer(&self);
}
```

Every type stored inside a `Gc<T>` must implement `Trace`. The collector calls `trace`
during the mark phase to discover reachable objects.

Example implementations:

- https://github.com/boa-dev/boa/blob/main/core/engine/src/object/jsobject.rs
- https://github.com/boa-dev/boa/blob/main/core/engine/src/context/mod.rs

### Finalize

```rust
pub trait Finalize {
    /// Called before the object is reclaimed.
    fn finalize(&self);
}
```

`Finalize` provides a cleanup hook that runs **before** memory is freed. Implementations
may perform resource cleanup (e.g., releasing file handles or detaching array buffers).

> **Important:** Finalizers execute before the sweep phase and may resurrect objects by
> storing references back into the live graph. The collector must re-mark the heap after
> finalization to handle this case.

---

## 7. Derive Macros

### Automatic Derivation

```rust
#[derive(Trace, Finalize)]
pub struct MyStruct {
    field: Gc<OtherStruct>,
}
```

`#[derive(Trace)]` generates a `trace` implementation that recursively traces each field.
`#[derive(Finalize)]` generates an empty finalizer.

### Manual Tracing Helpers

```rust
// Implement Trace with a custom body.
custom_trace!(this, mark, {
    mark(&this.field_a);
    mark(&this.field_b);
});

// Implement Trace as a no-op for types containing no GC pointers.
empty_trace!();

// Unsafe variant of empty_trace for foreign types.
unsafe_empty_trace!();
```

---

## 8. Tracing Infrastructure

The `Tracer` type is passed to every `Trace::trace` implementation during the mark phase.

```rust
impl Tracer {
    /// Enqueue a GC pointer for marking.
    pub fn enqueue(&mut self, ptr: GcErasedPointer);
}
```

The `custom_trace!` macro provides a `mark` closure that calls `tracer.enqueue` internally,
so most code interacts with the tracer indirectly:

```rust
custom_trace!(this, mark, {
    mark(&this.some_gc_field);
});
```

The tracer uses an internal work queue to avoid deep recursion when walking large object
graphs.

---

## 9. Weak Collections

### WeakMap

```rust
WeakMap::new()  -> WeakMap<K, V>
WeakMap::get(&self, key: &K)  -> Option<V>
WeakMap::set(&self, key: &K, value: V)
WeakMap::has(&self, key: &K)  -> bool
WeakMap::delete(&self, key: &K) -> bool
WeakMap::get_or_insert(&self, key: &K, value: V) -> V
WeakMap::get_or_insert_computed(&self, key: &K, f: F) -> V
```

### Ephemeron Rule

> A value in an ephemeron pair remains reachable **only if** the key is independently
> reachable through the strong reference graph.

The collector must implement this rule during the mark phase:
1. Mark all strong roots.
2. For each ephemeron, if the key is marked, trace the value.
3. Repeat until no new marks are produced.
4. Remaining ephemerons with unmarked keys are considered dead.

---

## 10. Runtime Utilities

```rust
/// Trigger an immediate garbage collection cycle.
pub fn force_collect();

/// Returns true if it is safe to run finalizers (i.e., the collector is not
/// currently in the sweep/drop phase).
pub fn finalizer_safe() -> bool;
```

`force_collect` is used by tests, the CLI debugger, and indirectly by `WeakRef` deref
semantics. `finalizer_safe` guards against use-after-free during the drop phase.

---

## 11. Allocation Model

All GC allocations go through `Gc::new`:

```rust
let obj = Gc::new(MyValue { ... });
let cell = Gc::new(GcRefCell::new(inner));
```

**No heap handle is passed.** The GC runtime manages heap state internally (typically
via thread-local storage). This means a replacement collector must either:
- use a thread-local or global allocator, **or**
- refactor the engine to pass an explicit context (breaking change).

---

## 12. Minimal Compatibility Contract

### Pointer Types
- `Gc<T>`, `WeakGc<T>`, `GcRefCell<T>`, `GcRef<T>`, `GcRefMut<T>`

### Traits
- `Trace`, `Finalize`

### Derive & Helper Macros
- `#[derive(Trace)]`, `#[derive(Finalize)]`
- `custom_trace!`, `empty_trace!`, `unsafe_empty_trace!`

### Pointer Methods
- `Gc::new`, `Gc::new_cyclic`, `Gc::into_raw`, `Gc::from_raw`
- `Gc::ptr_eq`, `Gc::downcast`, `Gc::cast_unchecked`, `Gc::cast_ref_unchecked`
- `Clone`, `Deref`

### Interior Mutability
- `GcRefCell::new`, `borrow`, `borrow_mut`, `try_borrow`, `try_borrow_mut`, `into_inner`
- `GcRef::map`, `GcRef::try_map`, `GcRef::cast`
- `GcRefMut::map`, `GcRefMut::try_map`, `GcRefMut::cast`

### Weak References
- `WeakGc::new`, `WeakGc::upgrade`
- `WeakMap` with full CRUD API
- Ephemeron semantics

### Runtime Utilities
- `force_collect()`, `finalizer_safe()`

---

## 13. Conclusion

Boa relies on a small but precise garbage collector interface organized around five
concepts:

1. **Strong pointers** (`Gc<T>`) for all heap-allocated engine objects.
2. **Weak references** (`WeakGc<T>`, `Ephemeron`, `WeakMap`) for caches and JS weak
   collections.
3. **Interior mutability** (`GcRefCell<T>`) for safe mutation behind shared pointers.
4. **Trait-based tracing** (`Trace`, `Finalize`) for reachability analysis and cleanup.
5. **Macro-generated traversal** (`#[derive(Trace)]`, `custom_trace!`) for ergonomic
   integration with 100+ engine types.

Any collector that implements this interface — with stable non-moving pointer identity
and correct ephemeron support — can serve as a drop-in replacement for `boa_gc`.
