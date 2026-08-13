//! The heap header wrapping every GC-managed value.

use core::any::TypeId;
use core::cell::Cell;
use core::ptr::NonNull;

use crate::alloc::mempool3::{PoolAllocator, PoolItem};
use crate::collectors::mark_sweep_branded::trace::{Trace, TraceFn, Tracer};

pub(crate) type DropFn = unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>);

/// The tri-color marking state of a [`GcBox`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum GcColor {
    /// Not yet reached by mark phase
    White = 0,
    /// Reached and queued in the worklist, children not yet traced.
    Gray = 1,
    /// Reached and dequeued from the worklist, all children traced
    Black = 2,
}

/// Heap wrapper for a garbage-collected value.
///
/// Allocated via [`PoolAllocator`].
pub struct GcBox<T: ?Sized> {
    /// tricolor marking state, updated by the mark phase
    pub(crate) color: Cell<GcColor>,
    /// Type-erased trace function.
    pub(crate) trace_fn: TraceFn,
    /// Type-erased finalize and free fn
    pub(crate) drop_fn: DropFn,
    /// Allocation ID used to validate weak pointers.
    pub(crate) alloc_id: usize,
    /// Unique identifier for the concrete type `T`.
    ///
    /// Stored as `typeid::of::<T>()`. This safely erases branded lifetimes
    /// (eg. `'gc`) without requiring `T: 'static`, giving us a stable
    /// unique identity guarantee for sound downcasting.
    pub(crate) type_id: TypeId,
    /// The user value.
    pub(crate) value: T,
}

impl<T: ?Sized> GcBox<T> {
    pub(crate) const FREED_ALLOC_ID: usize = usize::MAX;
}

impl<T: Trace> GcBox<T> {
    /// Create a [`GcBox`] for `value`, `color` starts as [`GcColor::White`].
    ///
    /// Requires `T: Trace` for the `TypeId`.
    pub(crate) fn new(value: T, trace_fn: TraceFn, drop_fn: DropFn, alloc_id: usize) -> Self {
        Self {
            color: Cell::new(GcColor::White),
            trace_fn,
            drop_fn,
            alloc_id,
            type_id: typeid::of::<T>(),
            value,
        }
    }
}

/// type-erased trace function for a `GcBox<T>` slot.
///
/// # Safety
///
/// `ptr` must point to a live `PoolItem<GcBox<T>>` in the pool allocator
pub(crate) unsafe fn trace_value<T: Trace>(ptr: NonNull<u8>, tracer: &mut Tracer<'_>) {
    let pool_item_ptr = ptr.cast::<PoolItem<GcBox<T>>>();
    unsafe {
        (*pool_item_ptr.as_ptr()).0.color.set(GcColor::Black);
        (*pool_item_ptr.as_ptr()).0.value.trace(tracer);
    }
}
