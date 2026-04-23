//! `MutationContext<'id, 'gc>` handle.

use crate::collectors::mark_sweep_branded::{
    Collector,
    gc::{Gc, Root, RootNode},
    root_link::RootLink,
    trace::{Finalize, Trace},
    weak::WeakGc,
};
use core::marker::PhantomData;
use core::ptr::NonNull;
use rust_alloc::boxed::Box;

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
        let alloc_id = unsafe { (*gc.ptr.as_ptr()).alloc_id };
        WeakGc {
            ptr: gc.ptr,
            alloc_id,
            _marker: PhantomData,
        }
    }

    /// Promotes a `Gc` pointer to a `Root`
    pub fn root<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> Root<'id, T> {
        let gc_ptr = gc.ptr;

        let node = Box::new(RootNode {
            link: RootLink::new(),
            gc_ptr,
            _marker: PhantomData,
        });

        let raw = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

        // SAFETY: `raw` points to a stable `RootNode`.
        unsafe {
            let sentinel_ptr = NonNull::new_unchecked(self.collector.sentinel.as_ref().get_ref()
                as *const RootLink
                as *mut RootLink);
            let link_ptr = raw.cast::<RootLink>();
            RootLink::link_after(sentinel_ptr, link_ptr);
        }

        Root { raw }
    }

    /// Triggers a gc cycle.
    pub fn collect(&self) {
        self.collector.collect();
    }
}
