# Prototype Findings

Prototyping lifetime-branded GC API for Boa. Testing if `Gc<'gc, T>` + `Root<T>` is viable.

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
    global_object: Root<JsObject>,  // escapes 'gc
}
```

Root re-enters via `root.get(&cx)`.

### Collector ID Validation

Problem: `Root<T>` from collector A used with context B → dangling pointer.

Solution: Each collector gets unique ID, `Root` validates:

```rust
impl<T: Trace> Root<T> {
    pub fn get<'gc>(&self, cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        assert_eq!(self.collector_id, cx.collector.id);
        // ...
    }
}
```

Catches cross-collector misuse where lifetimes can't help.

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

### Root Cleanup

Problem: Root registered but never removed -> memory leak. Collector dropped before root -> UAF if roots were a raw pointer.

Solution: Roots use an **intrusive doubly linked list** with O(1) insertion/removal. Each `Root<T>` contains prev/next pointers, and `Pin<Box<Root<T>>>` ensures stable memory addresses. `Drop` unregisters in O(1):

```rust
pub struct Root<T: Trace + ?Sized> {
    gc_ptr: NonNull<GcBox<T>>,
    collector_id: u64,
    collector_roots: Rc<RefCell<RootList>>,
    node: RootListNode,  // Intrusive list node with prev/next pointers
    _marker: PhantomData<*const ()>,
}

impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        // O(1) removal from intrusive linked list
        unsafe {
            let node_ptr = NonNull::new_unchecked(&self.node as *const _ as *mut RootListNode);
            self.collector_roots.borrow().remove(node_ptr);
        }
    }
}
```

The `Pin` wrapper guarantees roots have stable addresses, which is conceptually correct (roots shouldn't move) and enables the intrusive list optimization.

**Previous approach** (Vec + retain): O(n²) worst case when dropping n roots (each scans entire list). Intrusive list is O(n) total.

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

**Compile-time isolation**: Borrow checker prevents mixing `Gc` from different contexts.

**Runtime cross-collector detection**: `Root::get()` panics on wrong collector.

**Root cleanup**: Drop removes from root list.

**Interior Mutability Tracing**: Using `GcRefCell<T>` allows `RefCell` semantics to persist efficiently while fulfilling `Trace` safety requirements without borrowing errors.

**Scopeless Weak Binding**: `WeakGc<T>` survives successfully unbranded and can trace/upgrade against an arbitrary temporal `MutationContext` when actively touched again.

**Functional Builtin Prototyping**: Explicit tests matching exactly against definitions like `Array.prototype.push` (taking a `&Gc<'gc, GcRefCell<JsArray<'gc>>>` + `arg` buffer bound to `_cx: &MutationContext<'gc>`) compiled gracefully and safely.

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

## Conclusion

`Gc<'gc, T>` + `Root<T>` is:
- **Sound**: Compile-time catches misuse
- **Runtime-safe**: Collector ID validation catches Root misuse
- **Fast**: Zero cost transient pointers
- **Feasible**: Can coexist with current API

Main risk is migration effort, we can go with the phased approach
