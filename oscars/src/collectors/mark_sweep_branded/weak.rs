//! `WeakGc<'id, T>` for weak references.

use crate::collectors::mark_sweep_branded::{
    gc::Gc,
    gc_box::GcBox,
    trace::{Finalize, Trace},
};
use core::marker::PhantomData;
use core::ptr::NonNull;

/// A weak reference to a GC managed value
pub struct WeakGc<'id, T: Trace + ?Sized> {
    pub(crate) ptr: NonNull<GcBox<T>>,
    pub(crate) alloc_id: usize,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace> WeakGc<'id, T> {
    /// Attempts to upgrade to a strong `Gc<'gc, T>`.
    pub fn upgrade<'gc>(
        &self,
        _cx: &crate::collectors::mark_sweep_branded::MutationContext<'id, 'gc>,
    ) -> Option<Gc<'gc, T>> {
        // SAFETY: `_cx` proves the `Collector` is alive.
        // `alloc_id` confirms the allocation is still valid.
        // The allocator does not unmap memory, so reading a recycled block's `alloc_id` is safe
        let is_valid = unsafe { (*self.ptr.as_ptr()).alloc_id == self.alloc_id };

        if is_valid {
            Some(Gc {
                ptr: self.ptr,
                _marker: PhantomData,
            })
        } else {
            None
        }
    }
}

impl<'id, T: Trace + ?Sized> Clone for WeakGc<'id, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'id, T: Trace + ?Sized> Copy for WeakGc<'id, T> {}

impl<'id, T: Trace> Finalize for WeakGc<'id, T> {}
impl<'id, T: Trace> Trace for WeakGc<'id, T> {
    // Weak references do not mark their target; upgrade() returning None after collection is the intended behaviour.
    fn trace(&mut self, _tracer: &mut crate::collectors::mark_sweep_branded::trace::Tracer) {}
}
