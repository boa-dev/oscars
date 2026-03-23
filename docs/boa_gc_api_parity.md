# Boa GC API Parity Matrix

## Overview

This document maps the current Boa-facing `boa_gc` API surface from
[`docs/boa_gc_api_surface.md`](./boa_gc_api_surface.md) onto the current
`oscars::mark_sweep` implementation.

The goal is not to restate the `boa_gc` surface. The goal is to answer a more
practical integration question:

> If Boa were wired to Oscars today, which pieces of the engine-facing GC
> contract are already available, which are only partially available, and which
> still need new API work?

This is intended to support:

- `oscars#26` Tracking issue for Boa integration
- `oscars#28` Integration into Boa
- the staged precise-tracing redesign direction discussed in this repository

## Status legend

- `Implemented`: Oscars already exposes an equivalent API or behavior.
- `Partial`: close in spirit, but naming, call shape, or semantics still differ.
- `Missing`: not currently exposed by the active `oscars::mark_sweep` API.
- `Different by design`: present, but intentionally shaped around a different
  collector model that Boa cannot consume directly yet.

## Main integration blockers

The current blockers are mostly API-shape blockers, not collector-core blockers:

1. **Allocation still requires an explicit collector handle.**
   Boa's current `boa_gc` contract assumes `Gc::new(value)` with implicit GC
   state, while Oscars currently uses `Gc::new_in(value, collector)`.
2. **Weak reference API is not yet Boa-compatible.**
   Oscars exposes `WeakGc::new_in` and `value()`, while Boa expects
   `WeakGc::new` and `upgrade()`.
3. **Several pointer convenience operations are still missing.**
   Raw-pointer conversion, downcast helpers, pointer identity helpers, and
   `new_cyclic` are not exposed on the current Oscars `Gc<T>`.
4. **Runtime utility hooks are missing.**
   Boa currently relies on `force_collect()` and `finalizer_safe()`.
5. **Macro compatibility is not complete.**
   `empty_trace!` and `custom_trace!` exist, but `unsafe_empty_trace!` is not
   currently exposed in the active `mark_sweep` module.

Those gaps are the parts that most directly gate `oscars#28`.

## Core pointer types

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `Gc<T>` | `Partial` | Strong GC pointer exists, but allocation still goes through `Gc::new_in(value, collector)` instead of `Gc::new(value)`. | `oscars/src/collectors/mark_sweep/pointers/gc.rs` |
| `WeakGc<T>` | `Partial` | Weak pointer exists, but the public API is `new_in` + `value()` rather than `new` + `upgrade()`. | `oscars/src/collectors/mark_sweep/pointers/weak.rs` |
| `WeakMap<K, V>` | `Implemented` | GC-tracked weak map exists with insert/get/remove and collector-managed pruning. | `oscars/src/collectors/mark_sweep/pointers/weak_map.rs` |
| `Ephemeron<K, V>` | `Implemented` | Ephemeron support exists internally and is part of the weak semantics model. | `oscars/src/collectors/mark_sweep/internals/ephemeron.rs` |
| `GcRefCell<T>` | `Implemented` | Interior mutability wrapper exists with borrow APIs. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRef<'a, T>` | `Implemented` | Immutable borrow guard exists with mapping helpers. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefMut<'a, T>` | `Implemented` | Mutable borrow guard exists with mapping helpers. | `oscars/src/collectors/mark_sweep/cell.rs` |

## Pointer operations

### `Gc<T>` allocation and identity

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `Gc::new(value)` | `Different by design` | Oscars currently requires explicit collector threading via `Gc::new_in(value, collector)`. This is the largest Boa-facing API mismatch. | `oscars/src/collectors/mark_sweep/pointers/gc.rs` |
| `Gc::new_cyclic(...)` | `Missing` | No equivalent helper is currently exposed on `oscars::mark_sweep::Gc<T>`. | - |
| `Clone for Gc<T>` | `Implemented` | Clone exists and increments root tracking. | `oscars/src/collectors/mark_sweep/pointers/gc.rs` |
| `Deref for Gc<T>` | `Implemented` | `Gc<T>` dereferences to `T`. | `oscars/src/collectors/mark_sweep/pointers/gc.rs` |
| `Gc::ptr_eq(...)` | `Missing` | No public pointer-identity helper is currently exposed. | - |

### Raw pointer conversion and casting

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `Gc::into_raw(...)` | `Missing` | Raw pointer round-trip helpers are not exposed on the public `Gc<T>` API. | - |
| `Gc::from_raw(...)` | `Missing` | Same as above. | - |
| `Gc::downcast(...)` | `Missing` | Oscars exposes `type_id()` / `is::<U>()`, but not the Boa-style consuming downcast API. | `oscars/src/collectors/mark_sweep/pointers/gc.rs` |
| `Gc::cast_unchecked(...)` | `Missing` | No public unchecked cast helper currently exists. | - |
| `Gc::cast_ref_unchecked(...)` | `Missing` | No public unchecked reference cast helper currently exists. | - |

## Interior mutability parity

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `GcRefCell::new` | `Implemented` | Present with matching purpose. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefCell::borrow` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefCell::borrow_mut` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefCell::try_borrow` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefCell::try_borrow_mut` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefCell::into_inner` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRef::map` / `try_map` / `cast` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |
| `GcRefMut::map` / `try_map` / `cast` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/cell.rs` |

The interior mutability surface is one of the stronger parity areas today.

## Weak reference system parity

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `WeakGc::new(&Gc<T>)` | `Partial` | Supported in spirit as `WeakGc::new_in(&Gc<T>, collector)`. | `oscars/src/collectors/mark_sweep/pointers/weak.rs` |
| `WeakGc::upgrade()` | `Missing` | Current Oscars API exposes `value() -> Option<&T>` instead of returning `Option<Gc<T>>`. | `oscars/src/collectors/mark_sweep/pointers/weak.rs` |
| `WeakMap::new` / `insert` / `get` / `remove` | `Partial` | CRUD exists, but constructor and insertion also require an explicit collector handle. | `oscars/src/collectors/mark_sweep/pointers/weak_map.rs` |
| Ephemeron reachability semantics | `Implemented` | Collector owns ephemeron lifecycle and prunes based on key reachability. | `oscars/src/collectors/mark_sweep/internals/ephemeron.rs`, `oscars/src/collectors/mark_sweep/mod.rs` |

The weak semantics work is already substantial, but the public API still needs
to be made Boa-compatible.

## Traits and macros

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `Finalize` trait | `Implemented` | Present and used throughout the mark-sweep implementation. | `oscars/src/collectors/mark_sweep/trace.rs` |
| `Trace` trait | `Implemented` | Present and forms the collector traversal contract. | `oscars/src/collectors/mark_sweep/trace.rs` |
| `#[derive(Trace)]` / `#[derive(Finalize)]` | `Implemented` | Re-exported through `oscars_derive` under the `mark_sweep` feature. | `oscars/src/lib.rs` |
| `custom_trace!` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/trace.rs` |
| `empty_trace!` | `Implemented` | Present. | `oscars/src/collectors/mark_sweep/trace.rs` |
| `unsafe_empty_trace!` | `Missing` | No matching helper macro is currently exposed in the active `mark_sweep` API. | - |

## Runtime utility parity

| `boa_gc` surface | Oscars status | Notes | Current Oscars reference |
|---|---|---|---|
| `force_collect()` | `Missing` | Collection exists as a collector method (`collector.collect()`), but not as a Boa-style runtime utility. | `oscars/src/collectors/mark_sweep/mod.rs` |
| `finalizer_safe()` | `Missing` | No public equivalent hook is currently exposed. | - |

These are small APIs, but they matter for Boa integration because they are part
of the current engine contract.

## Additional Oscars-only APIs

Oscars already exposes some useful APIs that are not part of the current
`boa_gc` contract:

- collector configuration helpers such as `with_heap_threshold(...)`
- pool allocator visibility through `pools_len()`
- `Gc<T>::size()`, `type_id()`, and `is::<U>()`
- explicit collector traits (`Collector`) and collector-owned weak map tracking

These are useful for experimentation, but they do not reduce the current Boa
integration gaps on their own.

## Recommended staged follow-up

The current parity picture suggests a practical order for follow-up work:

1. **Close the public API gaps before deeper collector refactors.**
   The biggest blockers are API shape blockers, not missing sweep/trace
   machinery.
2. **Decide how Boa will obtain collector context.**
   Either Oscars grows a Boa-compatible allocation surface, or Boa adopts an
   explicit collector/context model under unstable feature flags.
3. **Normalize the weak API to match Boa expectations.**
   `upgrade()` semantics and Boa-compatible constructors are likely needed early
   for `WeakRef` / `WeakMap` integration.
4. **Add the small runtime utilities once the main pointer model is settled.**
   `force_collect()` and `finalizer_safe()` are integration glue, not the main
   redesign work.

## Summary

Oscars is already in a useful middle state:

- the core tracing/finalization model exists,
- weak structures and ephemerons exist,
- `GcRefCell` parity is relatively strong,
- but the public `Gc<T>` / `WeakGc<T>` / runtime utility layer is not yet
  Boa-compatible.

That means the next integration work should stay focused on **API parity and
adoption shape**, not just collector internals.
