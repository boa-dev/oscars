//! `MutationContext<'id, 'gc>` handle.

use crate::{
    alloc::mempool3::{PoolAllocError, PoolPointer},
    collectors::mark_sweep_branded::{
        Collector,
        ephemeron::Ephemeron,
        gc::Gc,
        gc_box::GcBox,
        root::Root,
        trace::{Finalize, Trace},
        weak::WeakGc,
    },
};
use core::marker::PhantomData;

/// Handle for GC allocations
pub struct MutationContext<'id, 'gc> {
    pub(crate) collector: &'gc Collector,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, 'gc> MutationContext<'id, 'gc> {
    /// Allocates a value on the GC heap.
    pub fn try_alloc<T: Trace + Finalize + 'gc>(
        &self,
        value: T,
    ) -> Result<Gc<'gc, T>, PoolAllocError> {
        self.collector.try_alloc(value)
    }

    /// Downgrades a `Gc` into a weak reference
    pub fn alloc_weak<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> WeakGc<'id, T> {
        let alloc_id = unsafe { (*gc.ptr.as_ptr().as_ptr()).0.alloc_id };
        WeakGc {
            ptr: gc.ptr,
            alloc_id,
            _marker: PhantomData,
        }
    }

    /// Promotes a `Gc` pointer to a `Root`
    pub fn root<T: Trace + Finalize + 'gc>(
        &self,
        gc: Gc<'gc, T>,
    ) -> Result<Root<'id, T>, PoolAllocError> {
        let raw = self.collector.try_alloc_root_node(gc.ptr)?;
        Ok(Root { raw })
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
        let key_alloc_id = unsafe { (*key.ptr.as_ptr().as_ptr()).0.alloc_id };
        // SAFETY: GcBox<K> and GcBox<V> are erased to GcBox<()>, the collector
        // only reads the fixed size prefix fields via this pointer
        let erased_key: PoolPointer<'static, GcBox<()>> =
            unsafe { key.ptr.to_erased().to_typed_pool_pointer::<GcBox<()>>() };
        let erased_value: PoolPointer<'static, GcBox<()>> =
            unsafe { value.ptr.to_erased().to_typed_pool_pointer::<GcBox<()>>() };
        self.collector.register_ephemeron(erased_key, erased_value);
        Ephemeron {
            key_ptr: Some(key.ptr),
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
