# Custom Pointer Integration Blockers


This is a follow-up to our initial research on adding custom pointers to `oscars`. We've built the `mempool4` prototype to test out the `(pool_id, slot_idx)` custom pointer idea, so let's get into what we found.

The primary goal of this exercise was to see if a 32-bit stable coordinate could actually work for allocations, resolutions, and heap serialization.

## General notes

The implementation itself does appear to function correctly. We can allocate, we can safely resolve using a `'gc` branded context, and the serialization story is incredibly clean since the coordinates don't need any fixup passes after restarting.

However, there are a few caveats. If we want to make this custom pointer approach work with the existing `mark_sweep_branded` API, we run into some serious integration blockers.

### Major blocker: Loss of `Deref`

Right now, the existing `Gc<'gc, T>` uses raw physical pointers under the hood and implements the `Deref` trait. This makes it really nice to use: `obj.properties()` just works.

Because our custom pointer is just a 32-bit number, it can't safely implement `Deref`. The compiler has no idea where the memory actually is without asking the allocator. So every read has to become `cx.resolve(obj).properties()`.

Why is this a major blocker? We have hundreds of call sites across `builtins/`, `object/`, `vm/`, and `environments/` that rely on `Deref`. Migrating all of those introduces a lot of API friction.

There are a few ways around this:
1. We just bite the bullet and migrate all the code.
2. The Hybrid Approach: we keep using real pointers for `Gc` at runtime (so `Deref` still works), and we only convert them into Custom Pointers when we need to serialize or pin something.
3. We put the allocator in Thread Local Storage (TLS) so `Deref` can look it up behind the scenes.

### Major blocker: The `Trace` trait

Currently, the `Trace` trait passes a real memory address to the `Tracer`. 

With `CustomPtr`, it's just a `NonZeroU32`. The tracer sees it and does nothing. It can't follow the coordinate because it doesn't have access to the `PoolAllocator4`. If it can't follow it, it thinks the object is dead and frees it, causing UAF.

We'd have to either pass the allocator into the tracer (an additive change) or change the signature of `Trace` entirely (a massive breaking change). Note that the Hybrid Approach mentioned above also neatly sidesteps this issue, since the tracer would only ever see real pointers.

### Room for improvement

There are a couple other open questions around the integration:

1. **Write Barriers:** When we assign a new GC pointer (like `node.next = other_gc`), the GC needs to know. With `Deref`, we could intercept this. With a raw `u32`, we can't. We'd need an explicit write API on the context.
2. **Pinning:** We built custom pointers to make pinning easy, but we haven't actually specced out what a "pinned object" looks like in the allocator. 

## Conclusion

The core custom pointer concept may very well be a valid path forward, but it will be dependent on how we want to handle the loss of `Deref`.

If we choose the Hybrid Approach, we solve both the `Deref` ergonomics issue and the `Trace` issue, though we pay a small cost in runtime conversions. Otherwise, we have to commit to a massive API migration. We need to make this decision before moving ahead.
