// `trait Collector: Allocator` supertrait
//
// key design decisions:
// - `collect()` takes `&self` so it can run while collections borrow the collector
// - alloc methods accept raw values so the `GcBox` header gets its color
//   after any GC collections happen, preventing tracing bugs

use crate::alloc::arena3::ArenaPointer;
use crate::collectors::mark_sweep::{
    TraceColor,
    internals::{Ephemeron, GcBox},
    trace::Trace,
};

// when `gc_allocator` is on, collections can use the GC's arena directly
#[cfg(feature = "gc_allocator")]
pub trait Collector: allocator_api2::alloc::Allocator {
    // trigger a full collection cycle
    fn collect(&self);

    // returns the current trace color for newly allocated objects
    fn gc_color(&self) -> TraceColor;

    // Allocates a standard GC node for `value`, wrapping it in a `GcBox`
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_gc_node<T: Trace + 'static>(
        &self,
        value: T,
    ) -> Result<ArenaPointer<'static, GcBox<T>>, allocator_api2::alloc::AllocError>;

    // Allocates an ephemeron node pointing to an existing GC key, and a new value
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_ephemeron_node<K: Trace + 'static, V: Trace + 'static>(
        &self,
        key: &crate::collectors::mark_sweep::Gc<K>,
        value: V,
    ) -> Result<ArenaPointer<'static, Ephemeron<K, V>>, allocator_api2::alloc::AllocError>;
}

// used when `gc_allocator` feature is off
#[cfg(not(feature = "gc_allocator"))]
pub trait Collector {
    // trigger a full collection cycle
    fn collect(&self);

    // returns the current trace color for newly allocated objects
    fn gc_color(&self) -> TraceColor;

    // Allocates a standard GC node for `value`, wrapping it in a `GcBox`
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_gc_node<T: Trace + 'static>(
        &self,
        value: T,
    ) -> Result<ArenaPointer<'static, GcBox<T>>, crate::alloc::arena3::ArenaAllocError>;

    // Allocates an ephemeron node pointing to an existing GC key, and a new value
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_ephemeron_node<K: Trace + 'static, V: Trace + 'static>(
        &self,
        key: &crate::collectors::mark_sweep::Gc<K>,
        value: V,
    ) -> Result<ArenaPointer<'static, Ephemeron<K, V>>, crate::alloc::arena3::ArenaAllocError>;
}
