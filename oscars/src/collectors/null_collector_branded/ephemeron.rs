use crate::{
    alloc::mempool3::PoolPointer,
    collectors::null_collector_branded::{
        gc::Gc,
        gc_box::GcBox,
        mutation_ctx::MutationContext,
        trace::{Finalize, Trace, Tracer},
    },
};
use core::marker::PhantomData;

pub struct Ephemeron<'id, K: Trace, V: Trace> {
    pub(crate) key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
    pub(crate) value_ptr: PoolPointer<'static, GcBox<V>>,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, K: Trace, V: Trace> Ephemeron<'id, K, V> {
    pub(crate) fn new(
        key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
        value_ptr: PoolPointer<'static, GcBox<V>>,
    ) -> Self {
        Self {
            key_ptr,
            value_ptr,
            _marker: PhantomData,
        }
    }

    /// Returns the value if the key is alive.
    pub fn get_value<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        // In the null collector, everything stays alive until context drops.
        if self.key_ptr.is_some() {
            Some(Gc::with_pointer(self.value_ptr))
        } else {
            None
        }
    }
}

impl<'id, K: Trace, V: Trace> Clone for Ephemeron<'id, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'id, K: Trace, V: Trace> Copy for Ephemeron<'id, K, V> {}

impl<'id, K: Trace, V: Trace> Finalize for Ephemeron<'id, K, V> {}

unsafe impl<'id, K: Trace, V: Trace> Trace for Ephemeron<'id, K, V> {
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}
