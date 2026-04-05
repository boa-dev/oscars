use crate::gc::{Gc, GcBox, MutationContext};
use crate::trace::{Finalize, Trace, Tracer};
use core::marker::PhantomData;
use core::ptr::NonNull;

/// A weak GC pointer that does not prevent collection.
///
/// The invariant `'id` brand guarantees it cannot be upgraded in a different context
/// or escape the [`with_gc`][crate::gc::with_gc] closure that created it.
pub struct WeakGc<'id, T: Trace + ?Sized> {
    pub(crate) ptr: NonNull<GcBox<T>>,
    /// A strict `'id` marker that stops the compiler from secretly altering
    /// lifetimes to bypass context checks.
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace + ?Sized> WeakGc<'id, T> {
    /// Attempts to upgrade to a `Gc<'gc, T>`, returning `None` if collected.
    ///
    /// Requires `&MutationContext` to statically verify the correct realm and pool liveness.
    pub fn upgrade<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Option<Gc<'gc, T>> {
        // SAFETY:
        // - Passing `_cx` guarantees the main GC context is still alive,
        //   meaning the memory pool holding this object hasn't been freed yet.
        // - We only read the `marked` flag. Since it uses `Cell`, we can safely
        //   check it without breaking memory aliasing rules.
        unsafe {
            let marked = (*self.ptr.as_ptr()).marked.get();
            if marked {
                Some(Gc {
                    ptr: self.ptr,
                    _marker: PhantomData,
                })
            } else {
                None
            }
        }
    }
}

impl<'id, T: Trace + ?Sized> Clone for WeakGc<'id, T> {
    fn clone(&self) -> Self {
        Self {
            ptr: self.ptr,
            _marker: PhantomData,
        }
    }
}

pub struct WeakMap<K: Trace, V: Trace> {
    _marker: PhantomData<(K, V)>,
}

impl<K: Trace, V: Trace> WeakMap<K, V> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<K: Trace, V: Trace> Trace for WeakMap<K, V> {
    fn trace(&mut self, _tracer: &mut Tracer) {}
}
impl<K: Trace, V: Trace> Finalize for WeakMap<K, V> {}
