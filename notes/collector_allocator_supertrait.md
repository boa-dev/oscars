# `Collector: Allocator`: is it possible and is it safe?

Note author: shruti2522
date: 2026-03-03

Issue #11 raised two questions about making `Collector` a supertrait of
`Allocator`.

## Is it possible in a generic way across multiple collectors?

Mostly yes. The main issue is that `Allocator` requires `&self` while allocation
paths take `&mut self`. The fix in PR #15 was to store the arena as
`RefCell<ArenaAllocator<'static>>` directly on `MarkSweepGarbageCollector`, then
implement `unsafe impl allocator_api2::alloc::Allocator for MarkSweepGarbageCollector`
directly on the collector. `allocate()` calls `borrow_mut()` on the `RefCell` for
the duration of the allocation and drops the borrow immediately after. Because
mark sweep is non moving, raw pointers from `allocate()` stay valid

For compacting collectors it does not work without extra machinery. A compacting
collector moves objects, which silently invalidates raw pointers held in a
`Vec<T, GcAllocator>` buffer, those collectors would need a pinning mechanism or
a different allocator surface (not so sure about this). the supertrait is practical here specifically
because mark sweep is non moving

there is also a reentrancy problem, `allocate()` takes a mutable borrow on the
`RefCell`. If that triggers a collection pass, the second borrow panics at
runtime, fixed it by putting the threshold check and `collect()` call before the
allocator borrow is taken, not inside the arena.

## Is it even safe to do this and use collections that may not be properly designed for this use case?

It depends on whether the collection implements `Trace`

A `Vec<T, GcAllocator>` on the stack or in a rooted `GcRefCell` is fine. The
problem is when the vec is stored inside a GC managed object. The mark phase then
needs to trace into the vec's elements. If it holds `Gc<T>` pointers and the
collector is not told about them, those pointers look unreachable and get swept.

`GcAllocVec` solves this by implementing `Trace` and visiting its elements. Any
collection stored in the GC managed heap that holds `Gc<T>` values needs a
`Trace` wrapper, plain data rooted at `Root`/stack does not.

`Collector: Allocator` is safe as long as:

1. The collector is non moving
2. `deallocate` is a no-op or correctly releases arena memory. For a bump arena,
   doing nothing is fine
3. Any collection stored in the GC managed heap that holds `Gc<T>` values must implement `Trace`

the supertrait bench confirms this. `GcAllocVec` is always wrapped in
`Root` or accessed through `GcRefCell` that implements `Trace`, and the
sweep phase causes no issues
