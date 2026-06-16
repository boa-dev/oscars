use core::ptr::NonNull;

use crate::alloc::mempool3::{PoolAllocator, PoolItem};

pub(crate) type DropFn = unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>);

/// Heap wrapper for a garbage collected value.
///
/// Allocated via [`PoolAllocator`]
pub(crate) struct GcBox<T: ?Sized> {
    /// Type erased finalize and free fn
    pub(crate) drop_fn: DropFn,
    /// User value
    pub(crate) value: T,
}

impl<T> GcBox<T> {
    /// Create a [`GcBox`] for `value`
    pub(crate) fn new(value: T, drop_fn: DropFn) -> Self {
        Self { drop_fn, value }
    }
}
