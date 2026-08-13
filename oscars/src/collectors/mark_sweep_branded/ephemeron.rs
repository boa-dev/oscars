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

/// A weak key / strong value association.
///
/// The value is kept alive by the collector as long as the key is reachable
/// from a root. Once the key is swept, `get_value()` / `value()` return `None`
/// and the entry is eligible for cleanup.
pub struct Ephemeron<'id, K: Trace + ?Sized, V: Trace> {
    pub(crate) key_ptr: Option<PoolPointer<'static, GcBox<K>>>,
    pub(crate) key_alloc_id: usize,
    pub(crate) value_ptr: PoolPointer<'static, GcBox<V>>,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, K: Trace + ?Sized, V: Trace> Ephemeron<'id, K, V> {
    pub(crate) fn new_raw(
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

    /// Allocates a new ephemeron binding `key` to `value`.
    ///
    /// The value is kept alive by the collector until the key is swept.
    pub fn new_with_mc<'gc>(cx: &MutationContext<'id, 'gc>, key: &Gc<'gc, K>, value: V) -> Self
    where
        K: Finalize,
        V: Finalize,
    {
        cx.alloc_ephemeron(key, cx.try_alloc(value).expect("Ephemeron value alloc"))
    }

    /// Convenience alias matching the `null_collector_branded` API.
    #[inline]
    pub fn new<'gc>(cx: &MutationContext<'id, 'gc>, key: &Gc<'gc, K>, value: V) -> Self
    where
        K: Finalize,
        V: Finalize,
    {
        Self::new_with_mc(cx, key, value)
    }
}

impl<'id, K: Trace + ?Sized, V: Trace> Ephemeron<'id, K, V> {
    /// Returns the value if the key is still alive, or `None` if collected.
    pub fn get_value<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        let key_alive = self
            .key_ptr
            .is_some_and(|p| unsafe { (*p.as_ptr().as_ptr()).0.alloc_id == self.key_alloc_id });
        if key_alive {
            Some(Gc::with_pointer(self.value_ptr))
        } else {
            None
        }
    }

    /// Returns the key if still alive, or `None` if collected.
    pub fn key<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, K>> {
        let key_alive = self
            .key_ptr
            .is_some_and(|p| unsafe { (*p.as_ptr().as_ptr()).0.alloc_id == self.key_alloc_id });
        if key_alive {
            self.key_ptr.map(|ptr| Gc::with_pointer(ptr))
        } else {
            None
        }
    }

    /// Returns the value if the key is still alive, or `None` if collected.
    ///
    /// Alias for [`Self::get_value`] to match the `null_collector_branded` API.
    pub fn value<'gc>(&self, cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, V>> {
        self.get_value(cx)
    }

    /// Returns `true` if the key is still alive (the value is reachable).
    pub fn has_value(&self) -> bool {
        self.key_ptr
            .is_some_and(|p| unsafe { (*p.as_ptr().as_ptr()).0.alloc_id == self.key_alloc_id })
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
    // Ephemerons do not mark their key; liveness of the key is determined
    // by the GC independently. The value is marked via the GC's ephemeron
    // fixpoint phase in `Collector::collect`.
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}
