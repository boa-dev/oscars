use crate::{
    alloc::mempool3::PoolPointer,
    collectors::null_collector_branded::{
        gc::Gc,
        gc_box::GcBox,
        trace::{Finalize, Trace},
    },
};
use core::marker::PhantomData;

/// A weak reference to a GC managed value
pub struct WeakGc<'id, T: Trace + ?Sized> {
    pub(crate) ptr: PoolPointer<'static, GcBox<T>>,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace> WeakGc<'id, T> {
    /// Attempts to upgrade to a strong `Gc<'gc, T>`
    pub fn upgrade<'gc>(
        &self,
        _cx: &crate::collectors::null_collector_branded::MutationContext<'id, 'gc>,
    ) -> Option<Gc<'gc, T>> {
        // In the null collector, everything stays alive until context drops.
        Some(Gc {
            ptr: self.ptr,
            _marker: PhantomData,
        })
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
    fn trace(&mut self, _tracer: &mut crate::collectors::null_collector_branded::trace::Tracer) {}
}
