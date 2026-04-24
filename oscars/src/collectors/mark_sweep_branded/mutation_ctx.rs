//! `MutationContext<'id, 'gc>` handle.

use crate::collectors::mark_sweep_branded::{
    Collector,
    ephemeron::Ephemeron,
    gc::{Gc, Root},
    root_link::RootLink,
    trace::{Finalize, Trace},
    weak::WeakGc,
};
use core::marker::PhantomData;

/// Handle for GC allocations
pub struct MutationContext<'id, 'gc> {
    pub(crate) collector: &'gc Collector,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, 'gc> MutationContext<'id, 'gc> {
    /// Allocates a value on the GC heap.
    ///
    /// # Panics
    ///
    /// Panics if the pool allocator fails to allocate.
    pub fn alloc<T: Trace + Finalize + 'gc>(&self, value: T) -> Gc<'gc, T> {
        self.collector.alloc(value)
    }

    /// Downgrades a `Gc` into a weak reference
    pub fn alloc_weak<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> WeakGc<'id, T> {
        let alloc_id = unsafe { (*gc.ptr.as_ptr()).0.alloc_id };
        WeakGc {
            ptr: gc.ptr,
            alloc_id,
            _marker: PhantomData,
        }
    }

    /// Promotes a `Gc` pointer to a `Root`
    pub fn root<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> Root<'id, T> {
        let raw = self.collector.alloc_root_node(gc.ptr);

        // SAFETY: `raw` points to a stable `RootNode`.
        unsafe {
            let sentinel_ptr = self.collector.sentinel.as_ptr();
            let link_ptr = raw.cast::<RootLink>();
            RootLink::link_after(sentinel_ptr, link_ptr);
        }

        Root { raw }
    }

    /// Creates an ephemeron binding `key` to `value`.
    ///
    /// The value is kept alive by the collector as long as the key remains
    /// reachable from a root. Once the key is collected, `get_value` returns
    /// `None` and the value is eligible for collection on the next cycle.
    pub fn alloc_ephemeron<K: Trace + Finalize + 'gc, V: Trace + Finalize + 'gc>(
        &self,
        key: Gc<'gc, K>,
        value: Gc<'gc, V>,
    ) -> Ephemeron<'id, K, V> {
        let key_alloc_id = unsafe { (*key.ptr.as_ptr()).0.alloc_id };
        self.collector.register_ephemeron(
            key.ptr.cast::<u8>(),
            key_alloc_id,
            value.ptr.cast::<u8>(),
        );
        Ephemeron {
            key_ptr: key.ptr,
            key_alloc_id,
            value_ptr: value.ptr,
            _marker: core::marker::PhantomData,
        }
    }

    /// Triggers a gc cycle.
    pub fn collect(&self) {
        self.collector.collect();
    }
}
