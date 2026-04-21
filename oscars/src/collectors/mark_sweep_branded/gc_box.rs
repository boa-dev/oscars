//! The heap header wrapping every GC-managed value.

use core::cell::Cell;

use crate::collectors::mark_sweep_branded::trace::TraceFn;

/// Heap wrapper for a garbage-collected value.
///
/// Allocated via [`PoolAllocator`][crate::alloc::mempool3::PoolAllocator].
pub(crate) struct GcBox<T: ?Sized> {
    /// Reachability flag set by the mark phase.
    pub(crate) marked: Cell<bool>,
    /// Type-erased trace function.
    pub(crate) trace_fn: TraceFn,
    /// Allocation ID used to validate weak pointers.
    pub(crate) alloc_id: usize,
    /// The user value.
    pub(crate) value: T,
}

impl<T: ?Sized> GcBox<T> {
    pub(crate) const FREED_ALLOC_ID: usize = usize::MAX;
}
