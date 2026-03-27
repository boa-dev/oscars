// `WeakGc<T>` uses `Ephemeron<T, ()>`, this allocates two GcBox headers
// per weak pointer. This overhead is acceptable for now but could be
// optimized in the future
use crate::{
    alloc::mempool3::PoolPointer,
    collectors::mark_sweep::{Collector, Gc, Trace, internals::Ephemeron},
};

#[repr(transparent)]
pub struct WeakGc<T: Trace + 'static> {
    inner_ptr: PoolPointer<'static, Ephemeron<T, ()>>,
}

impl<T: Trace> WeakGc<T> {
    pub fn new_in<C: Collector>(value: &super::Gc<T>, collector: &C) -> Self
    where
        T: Sized,
    {
        let inner_ptr = collector
            .alloc_ephemeron_node(value, ())
            .expect("Failed to allocate Ephemeron node");

        // SAFETY: safe because the gc tracks this
        let inner_ptr = unsafe { inner_ptr.extend_lifetime() };

        Self { inner_ptr }
    }

    /// Returns the value of this [`WeakGc`] if the underlying value is alive.
    pub fn value(&self) -> Option<&T> {
        self.inner_ptr.as_inner_ref().key()
    }

    pub fn upgrade(&self) -> Option<Gc<T>> {
        self.inner_ptr.as_inner_ref().upgrade()
    }
}
