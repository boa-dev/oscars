# Conclusion: Key Learnings for Oscars GC Design

Note author: shruti2522

## Background

This research looked at Wasmtime's GC implementation to pull out useful lessons for `boa_gc` and Oscars. The goal was not to copy their design, but to learn from their decisions and apply the relevant parts to our own API redesign.

## What We Learned

### Keep the Collector Interface Internal

Wasmtime exposes a simple enum publicly but keeps the actual collector traits internal with no stability guarantees. We should do the same. Expose a simple config enum, keep collector traits private to `boa_gc`.

```rust
// Public
pub enum GcStrategy { Auto, MarkSweep, NullCollector }

// Internal (not public)
pub(crate) trait Collector {...}
```

This lets us add new collectors, change internal interfaces and swap the default without breaking the public API.

### Build the Null Collector First

Wasmtime built a bump allocator with no collection before writing any real collector code. This separated testing the object model from testing collection logic. We should do the same. The first milestone for Oscars is a `NullCollector` that traps on heap exhaustion. It lets us validate object headers and layout, run the full test suite and measure pure allocation overhead before adding any collection complexity.

### Precise Roots Are Not Optional

Wasmtime uses precise stack maps to track roots. We do not have a JIT but we still need precise roots. The handle table is the right approach for the prototype.

```rust
pub struct Context {
    handle_table: HandleTable,
}

pub struct Handle<T> {
    index: u32,
    _marker: PhantomData<T>,
}
```

Conservative scanning would block future moving and generational collectors. Start with precise roots now.

### Cycle Collection Cannot Wait

Wasmtime's DRC collector cannot collect cycles. For Wasm workloads this is acceptable. For Js it is not. JS creates cycles constantly through closures, prototype chains and event listeners. The collector has to handle cycles from day one. Mark sweep is the simplest path since it handles cycles naturally.

### Reserve Header Space Early

Define `GcHeader` now with reserved fields for future collectors. Adding header fields later means touching every allocation site in the codebase.

```rust
#[repr(C)]
pub struct GcHeader {
    shape_id: u32,
    gc_flags: u32,  // reserve even if only using 2 bits initially
}
```

The memory overhead is negligible. The redesign cost later is not.

### Separate Root Discovery from Collection

Root finding and the collection algorithm should be separate concerns. The collector receives a root set and works from there.

```rust
impl Context {
    fn collect_roots(&self) -> RootSet { /* scan handle table */ }
}

trait Collector {
    fn collect(&mut self, roots: &RootSet);
}
```

### Keep the Public API Safe

All unsafe code should live inside the boa_gc implementation, documented with `SAFETY:` comments and never surface at the public API boundary. Embedders and `boa_engine` callers should not need to write unsafe code to use the GC.

## What We Are Not Doing

No deferred reference counting. The cycle limitation is a dealbreaker for JS and the complexity is not worth it at this stage. No conservative stack scanning. No exposing collector traits publicly. No premature optimization,start with mark sweep and optimize once there is real data

## Recommended Order

1. Define internal `Collector` trait (keep private)
2. Implement `NullCollector` (bump allocator, no collection)
3. Validate the object model with tests
4. Build the handle table for precise root tracking
5. Implement `MarkSweepCollector` with cycle collection
6. Add cargo feature flags per collector
7. Optimize later with data

## Conclusion

Three things have to be right from the start: precise root tracking, cycle collection and reserved header space. Everything else can be improved over time. These three are very hard to add in later.

The useful thing from the Wasmtime research is not the DRC algorithm itself but the design patterns around it. Internal flexibility with a simple public API, separation of root discovery from collection and an incremental implementation path. Those apply regardless of which collector we use

## References

- Wasmtime DRC design: https://bytecodealliance.org/articles/reference-types-in-wasmtime
- Wasmtime GC RFC : https://github.com/bytecodealliance/rfcs/blob/main/accepted/wasm-gc.md
- Wasmtime Collector enum docs: https://docs.wasmtime.dev/api/wasmtime/enum.Collector.html
- Wasmtime proposal status: https://docs.wasmtime.dev/stability-wasm-proposals.html
- Wasmtime 27.0 release: https://bytecodealliance.org/articles/wasmtime-27.0
- New stack maps: https://bytecodealliance.org/articles/new-stack-maps-for-wasmtime
- WasmGC proposal: https://github.com/WebAssembly/gc/blob/main/proposals/gc/MVP.md
