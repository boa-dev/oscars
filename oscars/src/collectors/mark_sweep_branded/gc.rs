//! Core pointer types.

use crate::{
    alloc::mempool3::PoolPointer,
    collectors::mark_sweep_branded::{
        gc_box::GcBox,
        trace::{Finalize, Trace},
    },
};
use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;

/// A transient pointer to a GC-managed value.
#[derive(Debug)]
pub struct Gc<'gc, T: Trace + ?Sized + 'gc> {
    pub(crate) ptr: PoolPointer<'static, GcBox<T>>,
    pub(crate) _marker: PhantomData<(&'gc T, *const ())>,
}

impl<'gc, T: Trace + ?Sized + 'gc> Copy for Gc<'gc, T> {}
impl<'gc, T: Trace + ?Sized + 'gc> Clone for Gc<'gc, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Gc<'gc, T> {
    #[inline]
    pub(crate) fn with_pointer(ptr: PoolPointer<'static, GcBox<T>>) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
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
        unsafe { &(*self.ptr.as_ptr().as_ptr()).0.value }
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

impl<T: Trace> Finalize for Gc<'_, T> {}
impl<T: Trace> Trace for Gc<'_, T> {
    fn trace(&mut self, tracer: &mut crate::collectors::mark_sweep_branded::trace::Tracer) {
        tracer.mark(self);
    }
}
