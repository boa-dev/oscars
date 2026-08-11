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
    /// Creates an erased `MutationContext` tied to the provided `Collector`.
    ///
    /// # Safety
    /// The caller must ensure that the returned `MutationContext` (and any
    /// `Gc` pointers it creates) do not outlive the `Collector`.
    pub unsafe fn from_collector_erased(
        collector: &crate::collectors::mark_sweep_branded::Collector,
    ) -> MutationContext<'static, 'static> {
        let ptr = collector as *const _;
        MutationContext {
            collector: unsafe { &*ptr },
            _marker: PhantomData,
        }
    }
    /// Creates a global thread-local MutationContext for the mark sweep collector.
    ///
    /// **Note**: This is a temporary workaround to keep `boa_engine` working.
    /// It breaks the normal safety rules of the collector, and should only be
    /// used to support older code that relies on `Default`.
    #[cfg(feature = "std")]
    pub fn global() -> Self {
        std::thread_local! {
            static COLLECTOR: crate::collectors::mark_sweep_branded::Collector = crate::collectors::mark_sweep_branded::Collector::new();
        }
        COLLECTOR.with(|c| {
            let ptr = c as *const crate::collectors::mark_sweep_branded::Collector;
            Self {
                collector: unsafe { &*ptr },
                _marker: core::marker::PhantomData,
            }
        })
    }
    /// Allocates a value on the GC heap.
    pub fn try_alloc<T: Trace + Finalize + 'gc>(
        &self,
        value: T,
    ) -> Result<Gc<'gc, T>, PoolAllocError> {
        self.collector.try_alloc(value)
    }

    /// Downgrades a `Gc` into a weak reference
    pub fn alloc_weak<T: Trace + Finalize + ?Sized + 'gc>(
        &self,
        gc: &Gc<'gc, T>,
    ) -> WeakGc<'id, T> {
        let alloc_id = unsafe { (*gc.ptr.as_ptr().as_ptr()).0.alloc_id };
        WeakGc::with_pointer_and_alloc_id(gc.ptr, alloc_id)
    }

    /// Promotes a `Gc` pointer to a `Root`
    pub fn root<T: Trace + Finalize + 'gc>(
        &self,
        gc: Gc<'gc, T>,
    ) -> Result<Root<'id, T>, PoolAllocError> {
        let raw = self.collector.try_alloc_root_node(gc.ptr)?;
        Ok(Root::from_raw(raw))
    }

    /// Creates an ephemeron binding `key` to `value`.
    ///
    /// The value is kept alive by the collector as long as the key remains
    /// reachable from a root. Once the key is collected, `get_value` returns
    /// `None` and the value is eligible for collection on the next cycle.
    pub fn alloc_ephemeron<K: Trace + Finalize + ?Sized + 'gc, V: Trace + Finalize + 'gc>(
        &self,
        key: &Gc<'gc, K>,
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
        Ephemeron::new_raw(Some(key.ptr), key_alloc_id, value.ptr)
    }

    /// Triggers a gc cycle.
    pub fn collect(&self) {
        self.collector.collect();
    }

    /// Triggers a gc cycle, allowing external roots to be traced.
    pub fn collect_with_roots<F: FnOnce(&mut crate::collectors::mark_sweep_branded::Tracer)>(
        &self,
        trace_external: F,
    ) {
        self.collector.collect_with_roots(trace_external);
    }
}
