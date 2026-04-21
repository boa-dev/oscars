# Part 5: API Safety and Tracer Abstraction

Note author: shruti2522

## Background

Two things need to be sorted out before the prototype is usable: keeping the public API safe for embedders, and structuring the tracer so collection is efficient.

## API Safety

Wasmtime's GC RFC is clear that losing the safe by default API would be a failure. The approach is straightforward. All methods on GC managed objects take a `Store` or `Context` reference. `Store` is not `Sync`, so passing `&mut Store` proves you are on the right thread without needing locks. All unsafe code lives inside `wasmtime_runtime` and never surfaces to the public API.

The same idea applies to `boa_gc`. Embedders should not need to write unsafe code to use the GC.

```rust
pub struct Context {
    handle_table: HandleTable,
    collector: Box<dyn Collector>,
}

impl Context {
    pub fn allocate<T: Trace>(&mut self, value: T) -> Handle<T> {
        let ptr = self.collector.allocate(value);
        let index = self.handle_table.insert(ptr);
        Handle::new(index)
    }

    pub fn collect(&mut self) {
        self.collector.collect(&self.handle_table);
    }
}

impl !Sync for Context {}
impl !Send for Context {}
```

All `unsafe` blocks stay inside the boa_gc implementation, documented with `SAFETY:` comments and localized to allocator and collector code. An unsafe fast path can be offered for performance critical code but the default path is always safe.

## Tracer Abstraction

### The Problem with the Current Approach

Boa's current boa_gc uses a `Tracer` that collects reachable objects into a `Vec` while walking the object graph. There are a few issues with this. A flat Vec is tied to one traversal strategy, it does not distinguish between objects that are discovered, being traced or fully processed and growing a flat list is slow for large heaps.

### Separating Root Discovery from Collection

Wasmtime separates root discovery from collection. Root discovery builds the root set by walking the stack and scanning the activations table. Collection receives that root set and works from there. Changing the collector does not require changing root discovery and vice versa.

```rust
pub struct RootSet {
    roots: Vec<*mut GcHeader>,
}

impl Context {
    fn collect_roots(&self) -> RootSet {
        let mut roots = Vec::new();
        for ptr in self.handle_table.iter_live() {
            roots.push(ptr);
        }
        RootSet { roots }
    }
}

trait Collector {
    fn collect(&mut self, roots: &RootSet);
}
```

The tracing strategy stays internal to the collector. A tri-color work queue is more efficient than a flat `Vec`:

```rust
pub struct MarkSweepCollector {
    heap: Vec<GcBox<dyn Trace>>,
    grey_queue: VecDeque<*mut GcHeader>,
}

impl Collector for MarkSweepCollector {
    fn collect(&mut self, roots: &RootSet) {
        for &root in &roots.roots {
            self.mark(root);
        }
        while let Some(grey) = self.grey_queue.pop_front() {
            self.trace_children(grey);
        }
        self.sweep();
    }
}
```

White means not yet discovered. Grey means discovered but children not yet traced. Black means fully traced. Object state is always explicit.

### Trace Trait

The `Trace` trait should be simple:

```rust
pub trait Trace {
    fn trace(&self, tracer: &mut dyn FnMut(Gc<dyn Trace>));
}
```

A closure based tracer is simple to implement and lets the collector decide what to do with each child pointer. It does not lock you into a specific collection strategy.

## Conclusion

Keep the public API safe so embedders never need to write unsafe code. Separate root discovery from collection so each concern can change independently. Use a tri-color work queue inside the collector rather than a flat list. These decisions are straightforward to get right now and difficult to fix later.
