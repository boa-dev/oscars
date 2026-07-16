use crate::{
    alloc::mempool3::PoolPointer,
    collectors::mark_sweep_branded::{
        gc::Gc,
        gc_box::GcBox,
        mutation_ctx::MutationContext,
        trace::{Finalize, Trace, Tracer},
    },
};
use core::marker::PhantomData;

pub struct Ephemeron<'id, K: Trace, V: Trace> {
    pub(crate) key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
    pub(crate) key_alloc_id: usize,
    pub(crate) value_ptr: PoolPointer<'static, GcBox<V>>,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, K: Trace, V: Trace> Ephemeron<'id, K, V> {
    pub(crate) fn new(
        key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
        key_alloc_id: usize,
        value_ptr: PoolPointer<'static, GcBox<V>>,
    ) -> Self {
        Self {
            key_ptr,
            key_alloc_id,
            value_ptr,
            _marker: PhantomData,
        }
    }

    /// Returns the value if the key is alive.
    pub fn get_value<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        // SAFETY: `_cx` proves the collector is alive, alloc_id guards ABA
        let key_alive = self
            .key_ptr
            .is_some_and(|p| unsafe { (*p.as_ptr().as_ptr()).0.alloc_id == self.key_alloc_id });
        if key_alive {
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
