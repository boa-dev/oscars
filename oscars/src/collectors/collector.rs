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
    // the returned pointer is tied to the collector's lifetime
    fn alloc_gc_node<'gc, T: Trace + 'static>(
        &'gc self,
        value: T,
    ) -> Result<ArenaPointer<'gc, GcBox<T>>, allocator_api2::alloc::AllocError>;

    // Allocates an ephemeron node pointing to an existing GC key, and a new value
    //
    // The returned pointer is tied to the collector's lifetime
    fn alloc_ephemeron_node<'gc, K: Trace + 'static, V: Trace + 'static>(
        &'gc self,
        key: &crate::collectors::mark_sweep::Gc<K>,
        value: V,
    ) -> Result<ArenaPointer<'gc, Ephemeron<K, V>>, allocator_api2::alloc::AllocError>;

    // register a weak map with the GC so it can prune dead entries
    #[doc(hidden)]
    fn track_weak_map(
        &self,
        map: core::ptr::NonNull<dyn crate::collectors::mark_sweep::ErasedWeakMap>,
    );
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
    // the returned pointer is tied to the collector's lifetime.
    fn alloc_gc_node<'gc, T: Trace + 'static>(
        &'gc self,
        value: T,
    ) -> Result<ArenaPointer<'gc, GcBox<T>>, crate::alloc::arena3::ArenaAllocError>;

    // Allocates an ephemeron node pointing to an existing GC key, and a new value
    //
    // the returned pointer is tied to the collector's lifetime
    fn alloc_ephemeron_node<'gc, K: Trace + 'static, V: Trace + 'static>(
        &'gc self,
        key: &crate::collectors::mark_sweep::Gc<K>,
        value: V,
    ) -> Result<ArenaPointer<'gc, Ephemeron<K, V>>, crate::alloc::arena3::ArenaAllocError>;

    // Register a weak map with the GC so it can prune dead entries
    #[doc(hidden)]
    fn track_weak_map(
        &self,
        map: core::ptr::NonNull<dyn crate::collectors::mark_sweep::ErasedWeakMap>,
    );
}
