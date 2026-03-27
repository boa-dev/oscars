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

### Root Cleanup

Problem: Root registered but never removed → memory leak.

Solution: `Drop` unregisters:

```rust
impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        let roots = unsafe { &*self.collector_roots };
        roots.borrow_mut().retain(|e| e.ptr != self.ptr.as_ptr() as *mut u8);
    }
}
```

### !Send/!Sync

Single-threaded GC. Explicit bounds prevent cross-thread bugs.

## Validated

**Compile-time isolation**: Borrow checker prevents mixing `Gc` from different contexts.

**Runtime cross-collector detection**: `Root::get()` panics on wrong collector.

**Root cleanup**: Drop removes from root list.

### Performance

| Operation | Current | Proposed |
|-----------|---------|----------|
| `Gc::clone()` | Cell write | memcpy |
| `Gc::drop()` | Cell write | nothing |
| Root creation | N/A | O(1) |
| Root drop | N/A | O(n) |

## Challenges

**Collection timing**: When can GC run safely? Safe because all `Gc<'gc, T>` are on stack. Lifetime ensures no use after collection.

**FFI**: Native functions receive values but lifetimes don't cross FFI. Need handle scopes or root at boundary.

**Migration**: Boa has thousands of `Gc<T>` uses. Need to add `'gc` everywhere. Phasing gradually starting with isolated systems can be done

## Conclusion

`Gc<'gc, T>` + `Root<T>` is:
- **Sound**: Compile-time catches misuse
- **Runtime-safe**: Collector ID validation catches Root misuse
- **Fast**: Zero-cost transient pointers
- **Feasible**: Can coexist with current API

Main risk is migration effort, we can go with the phased approach
