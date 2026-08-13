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

pub struct Ephemeron<'id, K: Trace + ?Sized, V: Trace> {
    pub(crate) key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
    pub(crate) value_ptr: PoolPointer<'static, GcBox<V>>,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, K: Trace + ?Sized, V: Trace> Ephemeron<'id, K, V> {
    pub(crate) fn new_raw(
        key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
        value_ptr: PoolPointer<'static, GcBox<V>>,
    ) -> Self {
        Self {
            key_ptr,
            value_ptr,
            _marker: PhantomData,
        }
    }

    pub fn new<'gc>(cx: &MutationContext<'id, 'gc>, key: &Gc<'gc, K>, value: V) -> Self
    where
        V: Finalize,
    {
        let value_gc = Gc::new(cx, value);
        Self::new_raw(Some(key.ptr), value_gc.ptr)
    }

    pub fn get_value<'gc>(&self, cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        self.value(cx)
    }

    /// Returns the key if still live, or `None` if collected.
    ///
    /// Note: Always returns `Some` in the null collector.
    /// TODO: Return `None` when the key is only reachable through this ephemeron.
    pub fn key<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, K>> {
        self.key_ptr.map(|ptr| Gc::with_pointer(ptr))
    }

    /// Returns the value if the key is still live, or `None` if collected.
    ///
    /// Note: Always returns `Some` in the null collector.
    /// TODO: Return `None` when the key is collected.
    pub fn value<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        if self.key_ptr.is_some() {
            Some(Gc::with_pointer(self.value_ptr))
        } else {
            None
        }
    }

    /// Returns `true` if the key is still live.
    ///
    /// Note: Always returns `true` in the null collector.
    /// TODO: Return `false` once the collector clears `key_ptr`.
    pub fn has_value(&self) -> bool {
        self.key_ptr.is_some()
    }
}

impl<'id, K: Trace + ?Sized, V: Trace> Clone for Ephemeron<'id, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'id, K: Trace + ?Sized, V: Trace> Copy for Ephemeron<'id, K, V> {}

impl<'id, K: Trace + ?Sized, V: Trace> Finalize for Ephemeron<'id, K, V> {}

unsafe impl<'id, K: Trace + ?Sized, V: Trace> Trace for Ephemeron<'id, K, V> {
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}
