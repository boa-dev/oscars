use core::any::TypeId;
use core::ptr::NonNull;

use crate::alloc::mempool3::PoolAllocator;
use crate::collectors::null_collector_branded::trace::Trace;

pub type DropFn = unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>);

/// Heap wrapper for a garbage collected value.
///
/// Allocated via [`PoolAllocator`]
pub struct GcBox<T: ?Sized> {
    /// Type erased finalize and free fn
    pub(crate) drop_fn: DropFn,
    /// Unique identifier for the concrete type `T`.
    ///
    /// Stored as `typeid::of::<T>()`. This safely erases branded lifetimes
    /// (eg. `'gc`) without requiring `T: 'static`, giving us a stable
    /// unique identity guarantee for sound downcasting.
    pub(crate) type_id: TypeId,
    /// User value
    pub(crate) value: T,
}

impl<T: Trace> GcBox<T> {
    /// Create a [`GcBox`] for `value`.
    ///
    /// Requires `T: Trace` for the `TypeId`.
    pub(crate) fn new(value: T, drop_fn: DropFn) -> Self {
        Self {
            drop_fn,
            type_id: typeid::of::<T>(),
            value,
        }
    }
}
