//! The heap header wrapping every GC-managed value.

use core::cell::Cell;
use core::ptr::NonNull;

use crate::alloc::mempool3::PoolAllocator;
use crate::collectors::mark_sweep_branded::trace::TraceFn;

pub(crate) type DropFn = unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>);

/// Heap wrapper for a garbage-collected value.
///
/// Allocated via [`PoolAllocator`].
pub(crate) struct GcBox<T: ?Sized> {
    /// Reachability flag set by the mark phase.
    pub(crate) marked: Cell<bool>,
    /// Type-erased trace function.
    pub(crate) trace_fn: TraceFn,
    /// Type-erased finalize and free fn
    pub(crate) drop_fn: DropFn,
    /// Allocation ID used to validate weak pointers.
    pub(crate) alloc_id: usize,
    /// The user value.
    pub(crate) value: T,
}

impl<T: ?Sized> GcBox<T> {
    pub(crate) const FREED_ALLOC_ID: usize = usize::MAX;
}
