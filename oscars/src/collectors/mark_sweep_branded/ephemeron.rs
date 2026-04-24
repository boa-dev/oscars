use crate::{
    alloc::mempool3::PoolItem,
    collectors::mark_sweep_branded::{
        gc::Gc,
        gc_box::GcBox,
        mutation_ctx::MutationContext,
        trace::{Finalize, Trace, TraceColor},
    },
};
use core::marker::PhantomData;
use core::ptr::NonNull;

pub struct Ephemeron<'id, K: Trace, V: Trace> {
    pub(crate) key_ptr: NonNull<PoolItem<GcBox<K>>>,
    pub(crate) key_alloc_id: usize,
    pub(crate) value_ptr: NonNull<PoolItem<GcBox<V>>>,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, K: Trace, V: Trace> Ephemeron<'id, K, V> {
    /// Returns the value if the key is alive.
    pub fn get_value<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        // SAFETY: `_cx` proves the collector is alive; alloc_id guards ABA.
        let key_alive = unsafe { (*self.key_ptr.as_ptr()).0.alloc_id == self.key_alloc_id };
        if key_alive {
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
    fn trace(&self, _color: &TraceColor) {}
}
