# Prototype Findings

Prototyping lifetime-branded GC API for Boa. Testing if `Gc<'gc, T>` + `Root<'id, T>` is viable.

Works, but migration will be challenging.

## Current Oscars Model

```rust
// Clone/Drop touch root_count
impl<T: Trace> Clone for Gc<T> {
    fn clone(&self) -> Self {
        self.inner_ptr().as_inner_ref().inc_roots();
        // ...
    }
}
```

Every clone/drop modifies root count. Adds up in hot loops.

## Proposed Alternative

```rust
impl<'gc, T: Trace> Copy for Gc<'gc, T> {}
```

Zero cost. Lifetime proves validity.

## Design Decisions

### Lifetime Branding

**Runtime check**: `assert_eq!(self.context_id, CURRENT.get())` - cost on every access

**Lifetime**: `Gc<'gc, T>` - compiler enforces, zero runtime cost

### Interior Mutability

`&mut dyn GarbageCollector` breaks:

```rust
let a = cx.allocate(1);  // cx borrowed mutably
let b = cx.allocate(2);  // ERROR: still borrowed
```

Fix: `RefCell` inside collector, take `&self`.

### Explicit Rooting

`'gc` lifetime must end. Long-lived refs need escape hatch:

```rust
struct JsContext {
    global_object: Root<'id, JsObject>,  // escapes 'gc, tied to its GcContext<'id>
}
```

Root re-enters via `root.get(cx)` where `cx: &MutationContext<'id, 'gc>` must share the same `'id`.

### Cross-Context Safety via `'id` Brand

Problem: `Root<T>` from context A used with context B -> dangling pointer.

Solution: `with_gc` gives each context a fresh, unnamed `'id` lifetime via `for<'id>`. `Root<'id, T>` and `MutationContext<'id, 'gc>` share that brand, so the borrow checker rejects any mismatch at compile time:

```rust
impl<'id, T: Trace> Root<'id, T> {
    pub fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Gc<'gc, T> { ... }
}
```

No runtime check, no `collector_id` field, no atomic counter. The compiler does all the work.

### Gc Access Safety

**Q**: How do we prevent `Gc::get()` from accessing dead allocations?

Lifetime branding: `Gc<'gc, T>` can only exist within a `mutate()` closure and collection happens in the same scope via `cx.collect()`. The `'gc` lifetime ensures we can't hold a `Gc` pointer across a collection point. The compiler statically guarantees that all live `Gc<'gc, T>` values are on the stack during the `'gc` lifetime, so no runtime checks are needed in `Gc::get()`

```rust
ctx.mutate(|cx| {
    let obj = cx.alloc(JsObject { ... });  // Gc<'gc, JsObject>
    cx.collect();
    obj.get()  // Safe! 'gc lifetime proves it survived collection
});
// obj is gone here - 'gc lifetime ended
```

See compile-fail tests in `examples/api_prototype/tests/ui/` for examples of what the compiler prevents (escaping mutate(), cross context usage).

### Root Cleanup - `intrusive_collections` Design

Problem: Root registered but never removed -> memory leak. Collector dropped before root -> UAF if roots were a raw pointer.

Taking the `intrusive_collections` crate as inspiration, here is what we adopted and why:

#### What we adopted

1. **Pure Link Type (`RootLink`)**: Contains only `prev` and `next` pointers. No payload.
2. **O(1) Self Removal**: `unlink` drops nodes safely without a reference to the `Collector`.
3. **Double Unlink Protection**: `is_linked()` enforces safe dropping.
4. **Sentinel Node**: `Collector` owns a pinned `RootLink` as the list head.
5. **Type Erased Marking**: `RootNode<T>` is `#[repr(C)]` with `link` at offset 0. The GC walks the links and recovers `gc_ptr` using `offset_of!(RootNode<i32>, gc_ptr)`. A `debug_assert_eq!` with a second concrete type checks the offset is stable across all `T: Sized`. No `Trace` bound is needed.

#### Evolution of approaches

| Approach | Problem |
|---|---|
| `Vec` + `retain` | O(n^2) worst case to drop n roots |
| `Rc<RefCell<RootList>>` | Extra allocation and `Rc` clone per root |
| Impure link with `gc_ptr` inside | Mixes list logic with payload data |
| **Current: Pure `RootLink`** | O(1) operations, zero `Rc`, clean separation |


### Allocation Strategy

Prototype now uses `mempool3::PoolAllocator`:

- Size-class pooling with slot reuse
- O(1) allocation with cached slot pools
- O(log n) deallocation via sorted range index
- Arena recycling reduces OS allocation pressure
- Uses `try_alloc_bytes` for layout based allocation to support `'gc` lifetimes in user types


### !Send/!Sync

Single threaded GC. Explicit bounds prevent cross thread bugs.

## Validated

**Compile-time isolation**: Borrow checker prevents mixing `Gc`, `Root`, and `WeakGc` from different contexts. Cross-context use is a compile error, not a runtime panic.

**Root cleanup**: Drop unlinking removes from root list. `Box::from_raw` reclaims the node allocation.

**Interior Mutability Tracing**: Using `GcRefCell<T>` allows `RefCell` semantics to persist efficiently while fulfilling `Trace` safety requirements without borrowing errors.

**Branded Weak Binding**: `WeakGc<'id, T>` carries the same context brand. `upgrade` requires a matching `MutationContext<'id, 'gc>`, so cross-context upgrade is also a compile error.

**Functional Builtin Prototyping**: Explicit tests matching exactly against definitions like `Array.prototype.push` (taking a `&Gc<'gc, GcRefCell<JsArray<'gc>>>` + `arg` buffer bound to `_cx: &MutationContext<'id, 'gc>`) compiled gracefully and safely.

### Performance

| Operation | Current | Proposed |
|-----------|---------|----------|
| `Gc::clone()` | Cell write | memcpy |
| `Gc::drop()` | Cell write | nothing |
| Root creation | N/A | O(1) |
| Root drop | N/A | O(1) |

## Challenges

**Collection timing**: When can GC run safely? Safe because all `Gc<'gc, T>` are on stack. Lifetime ensures no use after collection.

**FFI**: Native functions receive values but lifetimes don't cross FFI. Need handle scopes or root at boundary.

**Migration**: Boa has thousands of `Gc<T>` uses. Need to add `'gc` everywhere. Phasing gradually starting with isolated systems can be done

### Root Node Stability via `Box::into_raw`

`Pin<Box<Root<T>>>` was the original approach: pinning kept the intrusive list node address stable.

The current approach is simpler: `cx.root()` allocates the node with `Box::new`, calls `Box::into_raw` immediately, and stores the raw `NonNull` inside a thin `Root<'id, T>` handle. The heap address is stable by construction. `Drop` calls `Box::from_raw` to reclaim it after unlinking.

This removes `Pin` from the public API entirely. `root()` returns `Root<'id, T>` (one word on the stack), not `Pin<Box<Root<T>>>`. The cost is still one heap allocation per escaping root, same as before.


## Conclusion

`Gc<'gc, T>` + `Root<'id, T>` is:
- **Sound**: Compile-time catches all cross-context misuse for `Gc`, `Root` and `WeakGc`
- **Fast**: Zero cost transient pointers, no atomic counters, no branch in `Root::get`
- **Feasible**: Can coexist with current API

Main risk is migration effort, we can go with the phased approach
