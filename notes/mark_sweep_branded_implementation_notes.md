# mark_sweep_branded Implementation Notes

**Date**: 2026-04-23  
**Status**: Production Ready

## Changes from API Redesign Proposal

### 1. Allocation ID for Weak References (ABA Protection)

**Added:**
- `alloc_id: usize` in `GcBox<T>` and `WeakGc<'id, T>`
- `FREED_ALLOC_ID = usize::MAX` constant
- Validation check in `WeakGc::upgrade`

**Why needed:**
Pool allocators reuse memory slots. Without IDs, a weak pointer could point to the wrong object after the slot is reused.

**How it works:**
- Each allocation gets a unique ID
- Freed slots get ID set to `usize::MAX`
- `WeakGc::upgrade` checks if IDs match
- If IDs don't match, slot was reused, return `None`

**Industry standard:**
V8 and SpiderMonkey use the same technique. Required for soundness with pool allocators.

### 2. Allocation ID Wrap Check

**Added:**
```rust
assert_ne!(alloc_id, FREED_ALLOC_ID, "...");
```

**Why:**
If the ID counter wraps to `usize::MAX`, weak reference validation breaks. This check prevents silent corruption.

**Practical impact:**
Requires 2^64 allocations on 64-bit systems (impossible in practice).

### 3. Additional Trace Implementations

**Added:**
- `BTreeMap<K, V>` (traces values only)
- `BTreeSet<T>` (no-op, keys are immutable)
- 3-tuple and 4-tuple
- Comments for `Rc<T>`, `Arc<T>`, `Cell<Option<T>>`

**Why:**
Needed for real Boa code. Keys in BTree collections are immutable, so they cannot contain `Gc` pointers (which need `&mut self` to trace).

**Note:**
`HashMap` and `HashSet` are in `std::collections`, not available in `no_std` builds.

### 4. Cell<Option<T>> Requires T: Copy

**Fixed:**
```rust
impl<T: Copy + Trace> Trace for Cell<Option<T>>
```

**Why:**
`self.set(Some(v))` requires moving `v`, which needs `T: Copy`. Without this bound, code fails to compile.

**Alternative:**
Use `GcRefCell<T>` for non-Copy types.

### 5. Documentation Improvements

**Added:**
- `Tracer<'a>` lifetime explanation
- `PoolAllocator<'static>` safety justification
- Comments on why certain impls are no-ops

## Design Decisions

### Trace::trace uses &mut self

Follows the proposal exactly. Allows future moving collectors to update internal pointers during tracing.

**Impact:**
Collection keys (HashMap, BTreeMap) cannot contain `Gc` pointers because keys are immutable.

### collect() uses &self not &mut self

Both `GcContext::collect` and `MutationContext::collect` use `&self` with interior mutability via `RefCell`.

**Why:**
Allows calling `collect()` inside `mutate()` closures without borrow conflicts.

## Testing

All tests pass:
- Unit tests (10 passed)
- Compile-fail tests (3 passed)
- Clippy (no warnings)
- Miri (no undefined behavior)
- Formatting (correct)

## Summary

Implementation is production ready and matches the approved API proposal. All additions are either:
- Required for soundness (ABA protection)
- Defensive checks (ID wrap)
- Practical needs (stdlib impls)
- Correctness fixes (Copy bounds)

No workarounds used. All unsafe code is justified.
