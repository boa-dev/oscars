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
    /// Returns the value if the key is alive.
    pub fn get_value<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        // In the null collector, everything stays alive until context drops.
        if self.key_ptr.is_some() {
            Some(Gc {
                ptr: self.value_ptr,
                _marker: PhantomData,
            })
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

impl<'id, K: Trace, V: Trace> Trace for Ephemeron<'id, K, V> {
    fn trace(&mut self, _tracer: &mut Tracer) {}
}
