# `Collector: Allocator`: is it possible and is it safe?

Note author: shruti2522
date: 2026-03-03

Issue #11 raised two questions about making `Collector` a supertrait of
`Allocator`

## Is it possible in a generic way across multiple collectors?

mostly yes, the main friction is that `Allocator` requires `&self` while allocation
paths take `&mut self`. The fix in PR #15 was to store the arena as
`RefCell<ArenaAllocator<'static>>` directly on `MarkSweepGarbageCollector`, then write `unsafe impl allocator_api2::alloc::Allocator for MarkSweepGarbageCollector`
. `allocate()` calls `borrow_mut()` on the `RefCell` for
the duration of the allocation and drops it right after. Because
mark sweep is non moving, raw pointers from `allocate()` stay valid

For compacting collectors it doesn't work without extra machinery, a compacting
collector moves objects around, which silently invalidates raw pointers held in a
`Vec<T, GcAllocator>` buffer, those would need a pinning mechanism or
a different allocator surface. The supertrait is practical here specifically
because mark sweep is non moving

There's also a reentrancy problem, `allocate()` takes a mutable borrow on the
`RefCell`. If that triggers a collection pass, the second borrow panics at
runtime. Fixed it by putting the threshold check and `collect()` call before the
allocator borrow, not inside the arena.

## Is it even safe to do this and use collections that may not be properly designed for this use case?

It depends on whether the collection implements `Trace`, a `Vec<T, GcAllocator>` on the stack or in a rooted `GcRefCell` is fine. The
problem is when the Vec lives inside a GC managed object. The mark phase needs to trace into the vec's elements, if it's holding `Gc<T>` pointers and the
collector doesn't know about them, those pointers look unreachable and get swept

`GcAllocVec` handles this by implementing `Trace` and visiting its elements. Any
collection stored in the GC managed object holds `Gc<T>` values needs a
`Trace` wrapper, plain data rooted at `Root` or on a stack doesn't.

`Collector: Allocator` is safe as long as:

1. The collector is non moving
2. `deallocate` is a no-op or correctly releases arena memory. For a bump arena doing nothing is fine
3. Any collection stored in the GC managed object that holds `Gc<T>` values must implement `Trace`

the `oscars_vs_boa_gc` bench confirms this holds in practice. `GcAllocVec` is always wrapped in
`Root` or accessed through `GcRefCell` that implements `Trace` and the
sweep phase causes no issues
