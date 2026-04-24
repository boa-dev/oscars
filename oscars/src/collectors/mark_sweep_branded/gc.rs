//! Core pointer types.

use crate::{
    alloc::mempool3::{PoolAllocator, PoolItem},
    collectors::mark_sweep_branded::{
        gc_box::GcBox,
        mutation_ctx::MutationContext,
        root_link::RootLink,
        trace::{Finalize, Trace},
    },
};
use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;
use core::ptr::NonNull;

pub(crate) type RootDropFn = unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>);

/// A transient pointer to a GC-managed value.
#[derive(Debug)]
pub struct Gc<'gc, T: Trace + ?Sized + 'gc> {
    pub(crate) ptr: NonNull<PoolItem<GcBox<T>>>,
    pub(crate) _marker: PhantomData<(&'gc T, *const ())>,
}

impl<'gc, T: Trace + ?Sized + 'gc> Copy for Gc<'gc, T> {}
impl<'gc, T: Trace + ?Sized + 'gc> Clone for Gc<'gc, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'gc, T: Trace + 'gc> Gc<'gc, T> {
    /// Returns a shared reference to the value.
    #[inline]
    pub fn get(&self) -> &T {
        // SAFETY: `ptr` is non-null and valid for `'gc` by construction.
        // The `'gc` lifetime is scoped to a `mutate()` closure, collection only occurs
        // via `cx.collect()` within that same closure and `Gc<'gc, T>` can't
        // escape the closure.
        unsafe { &(*self.ptr.as_ptr()).0.value }
    }
}

impl<'gc, T: Trace + fmt::Display + 'gc> fmt::Display for Gc<'gc, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.get(), f)
    }
}

impl<'gc, T: Trace + 'gc> Deref for Gc<'gc, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.get()
    }
}

/// Heap node backing a `Root`.
#[repr(C)]
pub(crate) struct RootNode<'id, T: Trace> {
    /// Intrusive list link
    pub(crate) link: RootLink,
    /// Pointer to the allocation
    pub(crate) gc_ptr: NonNull<PoolItem<GcBox<T>>>,
    /// Type-erased drop function for freeing this RootNode
    pub(crate) drop_fn: RootDropFn,
    /// Raw pointer to the Collector for freeing this node
    pub(crate) collector_ptr: *const crate::collectors::mark_sweep_branded::Collector,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

/// A handle that keeps a GC allocation live.
#[must_use = "dropping a root unregisters it from the GC"]
pub struct Root<'id, T: Trace> {
    pub(crate) raw: NonNull<RootNode<'id, T>>,
}

impl<'id, T: Trace> Root<'id, T> {
    /// Converts this root into a `Gc` pointer
    pub fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Gc<'gc, T> {
        Gc {
            // SAFETY: `raw` is non-null and valid.
            ptr: unsafe { self.raw.as_ref().gc_ptr },
            _marker: PhantomData,
        }
    }
}

impl<'id, T: Trace> Drop for Root<'id, T> {
    fn drop(&mut self) {
        unsafe {
            let node_ref = self.raw.as_ref();
            if node_ref.link.is_linked() {
                RootLink::unlink(NonNull::from(&node_ref.link));
            }
            // SAFETY: collector_ptr is valid for the lifetime of the GcContext
            let collector = &*node_ref.collector_ptr;
            collector.free_root_node(self.raw.cast::<u8>(), node_ref.drop_fn);
        }
    }
}

impl<T: Trace> Finalize for Gc<'_, T> {}
impl<T: Trace> Trace for Gc<'_, T> {
    fn trace(&self, color: &crate::collectors::mark_sweep_branded::trace::TraceColor) {
        color.mark(self);
    }
}
