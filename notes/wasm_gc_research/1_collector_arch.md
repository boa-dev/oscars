# Part 1: Collector Architecture Lessons from Wasmtime

Note author: shruti2522

## Background

Wasmtime recently shipped support for the Wasm GC proposal. They built a pluggable GC infrastructure in Rust to handle WasmGC structs and arrays. Wasmtime faces similar constraints to us, a safe Rust API, multiple collector options and adding GC onto an existing runtime that was not built with it in mind. Their architecture has some useful lessons.

## Keep Collector Traits Internal

Wasmtime has two internal traits for GC implementation, one used at compile time for object layout and write barrier insertion and one used at runtime for allocation and collection. These traits are explicitly not public API. Embedders never see them.

Instead, embedders pick a collector through a simple public enum. An `Auto` variant picks a sensible default, which can change between releases without breaking anything.

The same pattern makes sense for `boa_gc`:

```rust
// internal trait in boa_gc (not public)
pub(crate) trait Collector {
    fn allocate(&mut self, layout: Layout) -> *mut u8;
    fn collect(&mut self);
    fn write_barrier(&mut self, obj: GcBox<dyn Trace>);
}

// public configuration
pub enum GcStrategy {
    Auto,
    MarkSweep,
    NullCollector,
}
```

This lets us add new collectors, change internal interfaces and swap the `Auto` default without breaking the public API.

## Cargo Feature Flags Per Collector

Each collector in Wasmtime is behind its own cargo feature. This keeps builds with no GC at zero cost, lets embedded builds include only lightweight collectors and speeds up test compile times.

```toml
[features]
default = ["gc-mark-sweep"]
gc-mark-sweep = []
gc-null = []
gc-drc = []
```

Test suites can then compile with `--no-default-features --features gc-null` for faster builds and embedded targets can avoid pulling in a full collector.

## Build the Null Collector First

Wasmtime built a null collector (bump allocation, no collection) before implementing DRC. It traps when memory runs out. They did this to test the object model without needing a working collector, to get a performance baseline and because it is a legitimate option for very short lived programs.

The same approach works well for our prototype:

```rust
pub struct NullCollector {
    heap: BumpAllocator,
    limit: usize,
}
```

The benefits are straightforward. We can run the full test suite against the new allocator before writing any collection code, validate `GcHeader` layout and `Trace` implementations early and measure allocation overhead separately from collection overhead.

## Conclusion

There are three clear takeaways from Wasmtime's architecture. Keep the collector trait internal to `boa_gc`. Put each collector behind its own feature flag. Build and validate with the null collector before adding a real one. Following this order keeps each step independently testable and reduces the risk of getting deep into collection code before the object model is solid.
