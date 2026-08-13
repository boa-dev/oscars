use crate::{
    alloc::mempool3::PoolAllocError,
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
    /// Creates a global thread-local MutationContext for the null collector.
    ///
    /// **Note**: This is a temporary workaround to keep `boa_engine` working.
    /// It breaks the normal safety rules of the collector, and should only be
    /// used to support older code that relies on `Default`
    ///
    /// # Safety
    ///
    /// `Gc<'gc, T>` and `MutationContext` are both `!Send`, so neither can escape
    /// the thread that created them. `Collector::drop` only runs at thread exit,
    /// after which no `Gc` on this thread can be accessed. The raw pointer reborrow
    /// below is therefore sound ,the reference cannot outlive the TLS slot.
    #[cfg(feature = "std")]
    pub fn global() -> Self {
        std::thread_local! {
            static COLLECTOR: crate::collectors::null_collector_branded::Collector = crate::collectors::null_collector_branded::Collector::new();
        }
        COLLECTOR.with(|c| {
            // SAFETY: `Gc` and `MutationContext` are `!Send`, so they cannot escape
            // this thread. `COLLECTOR` is a thread-local whose destructor only runs
            // at thread exit, after all thread-local values are inaccessible.
            // Therefore, `c` remains valid for at least as long as any `MutationContext`
            // or `Gc` that could possibly reference it.
            let ptr = c as *const crate::collectors::null_collector_branded::Collector;
            Self {
                collector: unsafe { &*ptr },
                _marker: core::marker::PhantomData,
            }
        })
    }

    /// Creates a dummy `MutationContext` backed by a dangling pointer.
    ///
    /// # Safety
    ///
    /// The returned context is a placeholder and must never be used to
    /// allocate or access GC objects.
    pub unsafe fn dummy() -> Self {
        // SAFETY: We use a dangling pointer for the collector because it's a dummy.
        // It should never be used to allocate
        Self {
            collector: unsafe { &*core::ptr::NonNull::dangling().as_ptr() },
            _marker: PhantomData,
        }
    }

    /// Allocates a value on the GC heap
    pub fn try_alloc<T: Trace + Finalize + 'gc>(
        &self,
        value: T,
    ) -> Result<Gc<'gc, T>, PoolAllocError> {
        self.collector.try_alloc(value)
    }

    /// Downgrades a `Gc` into weak reference
    pub fn alloc_weak<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> WeakGc<'id, T> {
        WeakGc::with_pointer(gc.ptr)
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
    pub fn alloc_ephemeron<K: Trace + Finalize + ?Sized + 'gc, V: Trace + Finalize + 'gc>(
        &self,
        key: Gc<'gc, K>,
        value: Gc<'gc, V>,
    ) -> Ephemeron<'id, K, V> {
        // In the null collector, ephemerons don't need to be registered
        // since the collector never collects.
        Ephemeron::new_raw(Some(key.ptr), value.ptr)
    }

    pub fn collect(&self) {
        self.collector.collect();
    }
}
