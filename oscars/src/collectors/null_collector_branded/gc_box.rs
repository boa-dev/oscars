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
    /// Stored as `TypeId::of::<T::StaticId>()`, we use the `StaticId` proxy
    /// type so that branded lifetimes (eg. `'gc`) don't require `T: 'static`.
    /// Two values whose erased types share the same `StaticId` produce the
    /// same `TypeId`, which is exactly what we want for sound downcasting.
    pub(crate) type_id: TypeId,
    /// User value
    pub(crate) value: T,
}

impl<T: Trace> GcBox<T> {
    /// Create a [`GcBox`] for `value`.
    ///
    /// Requires `T: Trace` to access `T::StaticId` for the `TypeId`.
    pub(crate) fn new(value: T, drop_fn: DropFn) -> Self {
        Self {
            drop_fn,
            type_id: TypeId::of::<T::StaticId>(),
            value,
        }
    }
}
