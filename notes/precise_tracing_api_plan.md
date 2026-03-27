# Precise-Tracing API Redesign Proposal (Post `gc_allocator`)

Date: 2026-03-19

## Context

After removing the `Collector: Allocator` experiment (`#54`), the next step is
to propose a concrete API redesign shape that can be discussed and tested.

This note builds on:

- `notes/gc_api_models.md` (model-family investigation for boa#2631)
- `docs/boa_gc_api_surface.md` (current Boa-facing GC contract)
- Tracker issues `#26`, `#27`, `#28`, `#30`

The core target is a precise-tracing API that does not use root/reference
count arithmetic as the liveness authority.

## Problem statement

Today, `Gc<T>` ergonomics and liveness accounting are tightly coupled to root
counting. That gives simple usage but makes collector internals harder to
evolve and reason about.

For redesign work to be useful to Boa, we need an API that:

1. uses tracing as the single source of liveness truth,
2. keeps weak/ephemeron/finalizer semantics explicit,
3. remains adoptable against the current Boa-facing API surface.

## Proposed API (draft v0)

This proposal uses an explicit root-table model with scope handles.

### Core types

```rust
pub struct Gc<T: Trace + ?Sized> {
    ptr: GcErasedPointer,
    _marker: core::marker::PhantomData<T>,
}

pub struct Root<'scope, 'gc, T: Trace + ?Sized> {
    ptr: Gc<T>,
    slot: RootSlotId,
    _scope: core::marker::PhantomData<&'scope Scope<'gc>>,
}

pub struct WeakGc<T: Trace + ?Sized> {
    ptr: GcErasedPointer,
    _marker: core::marker::PhantomData<T>,
}

pub struct GcContext {
    /* collector state + root table + weak queues */
}

pub struct Scope<'gc> {
    cx: &'gc mut GcContext,
}
```

### Allocation and rooting

```rust
impl GcContext {
    pub fn scope<R>(&mut self, f: impl for<'gc> FnOnce(Scope<'gc>) -> R) -> R;
    pub fn collect(&mut self);
}

impl<'gc> Scope<'gc> {
    pub fn alloc<T: Trace + 'static>(&mut self, value: T) -> Gc<T>;
    pub fn root<'scope, T: Trace + 'static>(
        &'scope self,
        value: &Gc<T>,
    ) -> Root<'scope, 'gc, T>;
    pub fn downgrade<T: Trace + 'static>(&self, value: &Gc<T>) -> WeakGc<T>;
}

impl<'scope, 'gc, T: Trace + ?Sized> Root<'scope, 'gc, T> {
    pub fn gc(&self) -> &Gc<T>;
}
```

Safety note: `Root<'scope, 'gc, T>` is lifetime-branded to the borrowed scope,
so safe code cannot hold rooted references after the owning scope/context is
dropped.

Implementation note: `Scope::root(&self, ...)` assumes root-slot registration is
handled via internal mutability in collector internals, so multiple roots can
coexist without requiring an exclusive borrow of `Scope`.

### Pointer identity and casts (parity-preserving)

```rust
impl<T: Trace + ?Sized> Gc<T> {
    pub fn ptr_eq<U: Trace + ?Sized>(a: &Gc<T>, b: &Gc<U>) -> bool;
    pub fn into_raw(this: Gc<T>) -> GcRaw;
    pub unsafe fn from_raw(raw: GcRaw) -> Gc<T>;

    pub fn downcast<U: Trace + 'static>(this: Gc<T>) -> Option<Gc<U>>;
    pub unsafe fn cast_unchecked<U: Trace + 'static>(this: Gc<T>) -> Gc<U>;
    pub unsafe fn cast_ref_unchecked<U: Trace + 'static>(this: &Gc<T>) -> &Gc<U>;
}
```

### Weak behavior

```rust
impl<T: Trace + ?Sized> WeakGc<T> {
    pub fn new(value: &Gc<T>) -> WeakGc<T>;
    pub fn upgrade(&self) -> Option<Gc<T>>;
}
```

### Runtime helpers

```rust
impl GcContext {
    pub fn finalizer_safe(&self) -> bool;
}
```

`force_collect()` compatibility can be provided by wiring to
`MarkSweepGarbageCollector::collect` in integration mode.

## Semantics and invariants

### I1. Tracing is authoritative

Reachability is determined only by tracing from root slots and strong graph
edges in the same cycle.

### I2. Root slots replace root counts

A value is rooted when at least one root slot references it. Slot lifetime is
explicit (`Root<'scope, 'gc, T>` drop unregisters slot), and root handles are
branded by the scope/context lifetime so they cannot outlive the owning GC
context. No per-object root/refcount math is used for liveness.

### I3. Weak upgrade semantics

`WeakGc::upgrade` succeeds only if the referent is marked live in the current
collector state.

### I4. Ephemeron semantics

Ephemeron values are traced only when keys are independently reachable via
strong edges.

### I5. Finalizer ordering

Finalize-before-drop ordering is preserved, and collector teardown runs
finalizers before destructors for tracked live values.

### I6. Teardown safety

Collector drop does not free values in an order that can cause UAF via
finalizer-triggered graph activity.

## Feasibility and adoption path

This proposal is intentionally structured in two layers:

1. Collector-native API in Oscars (`GcContext`, `Scope`, `Root<'scope, 'gc, T>`).
2. Boa-compat layer that preserves current surface where required.

### Boa compatibility mapping

1. `Gc::new(value)`:
   - compatibility shim calls
     `with_gc_context(|cx| cx.scope(|mut s| s.alloc(value)))`.
2. `WeakGc::new/upgrade`, raw-pointer helpers, and cast helpers:
   - keep the same signatures.
3. `force_collect()`:
   - routed to collector `collect()`.
4. `finalizer_safe()`:
   - routed to collector phase state.

This keeps migration incremental and avoids an all-at-once engine rewrite.

## What this proposal deliberately does not include

1. A new incremental/generational/concurrent algorithm.
2. Full Boa integration in one milestone.
3. Allocator-framework redesign in the same proposal.

## Validation plan for this API proposal

1. Contract tests:
   - rooting slot lifetime and misuse resistance,
   - weak upgrade behavior across collections,
   - ephemeron key/value reachability behavior.
2. Safety checks:
   - Miri on root registration/unregistration paths and raw round-trips.
3. Compatibility checks:
   - parity checklist against `docs/boa_gc_api_surface.md`.
4. Benchmarks:
   - Boa workloads plus targeted GC stress cases.

## Open review questions

1. Should `Scope<'gc>` be mandatory for all allocations, or should we keep a
   global-context fallback for Boa compatibility?
2. Should `Root<'scope, 'gc, T>` be cloneable (multiple slots) or explicitly
   unique?
3. Is `cast_ref_unchecked` still desirable, or should compatibility rely on
   value-consuming casts only?
4. Which minimal Boa integration slice gives the best signal first:
   pointer API parity, weak semantics parity, or runtime helpers?
5. Should reading `T` from `Gc<T>` require an explicit scope/root token, so
   stale handles cannot be dereferenced after collection in safe code?

## Relationship to tracker work

This proposal is intended to feed directly into:

- `#26` Tracking issue for Boa integration
- `#27` Coverage of `boa_gc` API surface area
- `#28` Integration into Boa
- `#30` Benchmark MarkSweepGarbageCollector in Boa with `arena3` allocator

And it is aligned with `#54` (remove `gc_allocator` supertrait direction).
