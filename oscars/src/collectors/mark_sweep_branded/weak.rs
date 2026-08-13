//! `WeakGc<'id, T>` for weak references.

use crate::{
    alloc::mempool3::PoolPointer,
    collectors::mark_sweep_branded::{
        gc::Gc,
        gc_box::GcBox,
        trace::{Finalize, Trace},
    },
};
use core::marker::PhantomData;

/// A weak reference to a GC managed value
pub struct WeakGc<'id, T: Trace + ?Sized> {
    pub(crate) ptr: PoolPointer<'static, GcBox<T>>,
    pub(crate) alloc_id: usize,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace + ?Sized> WeakGc<'id, T> {
    pub(crate) fn with_pointer_and_alloc_id(
        ptr: PoolPointer<'static, GcBox<T>>,
        alloc_id: usize,
    ) -> Self {
        Self {
            ptr,
            alloc_id,
            _marker: PhantomData,
        }
    }

    /// Creates a new weak reference to a GC-managed value.
    ///
    /// This is a convenience wrapper over [`crate::collectors::mark_sweep_branded::MutationContext::alloc_weak`] that
    /// mirrors the `null_collector_branded` `WeakGc::new` API.
    #[inline]
    pub fn new<'gc>(
        cx: &crate::collectors::mark_sweep_branded::MutationContext<'id, 'gc>,
        value: &Gc<'gc, T>,
    ) -> Self
    where
        T: Finalize,
    {
        cx.alloc_weak(value)
    }

    /// Attempts to upgrade to a strong `Gc<'gc, T>`.
    pub fn upgrade<'gc>(
        &self,
        _cx: &crate::collectors::mark_sweep_branded::MutationContext<'id, 'gc>,
    ) -> Option<Gc<'gc, T>> {
        // SAFETY: `_cx` proves the `Collector` is alive.
        // `alloc_id` confirms the allocation is still valid.
        // The allocator does not unmap memory, so reading a recycled block's `alloc_id` is safe
        let is_valid = unsafe { (*self.ptr.as_ptr().as_ptr()).0.alloc_id == self.alloc_id };

        if is_valid {
            Some(Gc::with_pointer(self.ptr))
        } else {
            None
        }
    }

    /// Returns `true` if the referenced value is still alive.
    pub fn is_upgradable(&self) -> bool {
        unsafe { (*self.ptr.as_ptr().as_ptr()).0.alloc_id == self.alloc_id }
    }
}

impl<'id, T: Trace + ?Sized> Clone for WeakGc<'id, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'id, T: Trace + ?Sized> Copy for WeakGc<'id, T> {}

impl<'id, T: Trace + ?Sized> core::fmt::Debug for WeakGc<'id, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WeakGc")
            .field("ptr", &self.ptr.as_ptr())
            .field("alloc_id", &self.alloc_id)
            .finish()
    }
}

impl<'id, T: Trace + ?Sized> PartialEq for WeakGc<'id, T> {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::addr_eq(self.ptr.as_ptr().as_ptr(), other.ptr.as_ptr().as_ptr())
            && self.alloc_id == other.alloc_id
    }
}

impl<'id, T: Trace + ?Sized> Finalize for WeakGc<'id, T> {}
unsafe impl<'id, T: Trace + ?Sized> Trace for WeakGc<'id, T> {
    // Weak references do not mark their target, upgrade() returning None after collection is the intended behaviour.
    unsafe fn trace(&self, _tracer: &mut crate::collectors::mark_sweep_branded::trace::Tracer) {}
}
