# Part 2: Deferred Reference Counting Analysis

Note author: shruti2522

## Background

Wasmtime chose deferred reference counting (DRC) as their production GC. This note looks at why they made that choice and what we can take from it for boa_gc, even though we will likely go with mark sweep or something similar.

## Why Reference Counting?

Wasmtime picked reference counting over tracing GC for two reasons. First, refcounting spreads work across mutations rather than building up into large STW(stop the world) pauses. Second, the failure mode is safer. If tracing misses an object you get a dangling pointer. If refcounting misses one, we get a leak. Leaks are much easier to deal with than corruption when adding GC to an existing codebase.

The second point matters for oscars. Boa was not designed around GC from day one. A refcount bug causing a leak during development is a lot better than a crash.

## How Deferred Reference Counting Works

Standard refcounting would mean an increment and decrement on every assignment. For Wasm that means refcount operations on every `local.get` and `local.set`, which is too expensive.

Wasmtime avoids this by deferring. When a GC reference enters a Wasm frame it is inserted into `VMGcRefActivationsTable`. While Wasm runs, no refcount operations happen on local variables. Barriers only fire when a reference escapes the frame, such as being written to a struct field, global or table. Collection triggers when the activations table fills up. At that point Wasmtime walks the stack to find actually live objects and anything in the table that is no longer on the stack gets its refcount decremented.

The fast path for table insertion is very cheap. The slow path only runs when the table fills.

## The Cycle Problem

DRC cannot collect cycles. Objects that reference each other keep each other alive until the entire `Store` is dropped. Wasmtime accepts this because many Wasm programs are short lived.

For JavaScript this is not acceptable. JS creates cycles constantly. Closures capture their enclosing scope, prototype chains form back references, and event listeners hold references to the objects that registered them. A collector that cannot handle cycles will leak memory on almost any real program.

## Lessons for boa_gc

### Cycles Cannot Be Deferred

Unlike Wasmtime, we cannot ship something that leaks cycles and patch it later. Cycle collection has to be part of the initial design. The simplest path is to use mark sweep from the start, which handles cycles naturally.

```rust
impl MarkSweepCollector {
    fn collect(&mut self, roots: &RootSet) {
        for root in roots.iter() {
            self.mark(root);
        }

        self.heap.retain(|obj| {
            if obj.header.is_marked() {
                obj.header.unmark();
                true
            } else {
                false
            }
        });
    }
}
```

Generational collection, incremental marking and concurrent marking can all be layered on later once we have performance data.

### Do Not Optimize Early

Wasmtime's DRC design is complex because it targets Wasm's specific performance needs. We should not carry that complexity over. Start simple and optimize when there is data showing where the bottlenecks actually are.

### Deferred Barriers Are Still a Useful Idea

Even without full DRC, the idea of skipping barriers on local variables is worth keeping in mind. If roots are tracked precisely through a handle table, temporary allocations that never escape the current scope do not need write barriers at all. Collection only scans the handle table, so dropped local handles are automatically excluded.

## Conclusion

Do not implement DRC for the prototype. The complexity is not worth it and the cycle limitation is a dealbreaker for JavaScript.

Start with simple mark sweep, design precise root tracking from the start and defer optimization until there is real data to work from. The most useful thing to take from Wasmtime's DRC design is the principle of separating root tracking from the collection strategy not the algorithm itself.
