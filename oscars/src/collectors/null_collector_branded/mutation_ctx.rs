use crate::{
    alloc::mempool3::{PoolAllocError, PoolPointer},
    collectors::null_collector_branded::{
        Collector,
        ephemeron::Ephemeron,
        gc::Gc,
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
    /// Allocates a value on the GC heap
    pub fn try_alloc<T: Trace + Finalize + 'gc>(
        &self,
        value: T,
    ) -> Result<Gc<'gc, T>, PoolAllocError> {
        self.collector.try_alloc(value)
    }

    /// Downgrades a `Gc` into weak reference
    pub fn alloc_weak<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> WeakGc<'id, T> {
        WeakGc {
            ptr: gc.ptr,
            _marker: PhantomData,
        }
    }

    pub fn root<T: Trace + Finalize + 'gc>(
        &self,
        gc: Gc<'gc, T>,
    ) -> Result<Root<'id, T>, PoolAllocError> {
        Ok(Root::new(self, gc))
    }

    /// Creates an ephemeron binding `key` to `value`
    ///
    /// The value is kept alive by the collector as long as the key remains
    /// reachable from a root. Once the key is collected, `get_value` returns
    /// `None` and the value is eligible for collection on next cycle.
    pub fn alloc_ephemeron<K: Trace + Finalize + 'gc, V: Trace + Finalize + 'gc>(
        &self,
        key: Gc<'gc, K>,
        value: Gc<'gc, V>,
    ) -> Ephemeron<'id, K, V> {
        // In the null collector, ephemerons don't need to be registered
        // since the collector never collects.
        Ephemeron {
            key_ptr: Some(key.ptr),
            value_ptr: value.ptr,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn collect(&self) {
        self.collector.collect();
    }
}
