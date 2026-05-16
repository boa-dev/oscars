//! Interior mutability for GC-managed values.

use crate::collectors::mark_sweep_branded::trace::{Finalize, Trace, Tracer};
use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};

/// A GC-aware wrapper around [`RefCell<T>`].
pub struct GcRefCell<T: Trace> {
    inner: RefCell<T>,
}

impl<T: Trace> GcRefCell<T> {
    /// Wraps `value` in a new `GcRefCell`.
    pub fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
    }

    /// Acquires a shared borrow of the inner value.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently mutably borrowed.
    pub fn borrow(&self) -> GcRef<'_, T> {
        GcRef(self.inner.borrow())
    }

    /// Acquires a mutable borrow of the inner value.
    ///
    /// # Panics
    ///
    /// Panics if the value is currently borrowed.
    pub fn borrow_mut(&self) -> GcRefMut<'_, T> {
        GcRefMut(self.inner.borrow_mut())
    }
}

/// A shared borrow guard returned by [`GcRefCell::borrow`].
pub struct GcRef<'a, T: Trace>(Ref<'a, T>);

impl<T: Trace> Deref for GcRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// A mutable borrow guard returned by [`GcRefCell::borrow_mut`].
pub struct GcRefMut<'a, T: Trace>(RefMut<'a, T>);

impl<T: Trace> Deref for GcRefMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Trace> DerefMut for GcRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Trace> Finalize for GcRefCell<T> {}

impl<T: Trace> Trace for GcRefCell<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        self.inner.get_mut().trace(tracer);
    }
}
