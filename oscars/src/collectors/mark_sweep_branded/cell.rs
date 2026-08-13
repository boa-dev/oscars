//! Interior mutability for GC-managed values.

use crate::collectors::mark_sweep_branded::trace::{Finalize, Trace, Tracer};
use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};

/// A GC-aware wrapper around [`RefCell<T>`].
///
/// Unlike a plain `RefCell`, this can hold unsized `T` through a `Box<T>`
/// indirection when needed. The `T: Trace` bound ensures the GC can visit
/// the contained value.
pub struct GcRefCell<T: Trace + ?Sized> {
    inner: RefCell<T>,
}

impl<T: Trace> GcRefCell<T> {
    /// Wraps `value` in a new `GcRefCell`.
    pub fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
    }
}

impl<T: Trace + Clone> Clone for GcRefCell<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Trace + core::fmt::Debug + ?Sized> core::fmt::Debug for GcRefCell<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.inner, f)
    }
}

impl<T: Trace + Default> Default for GcRefCell<T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Trace + ?Sized> GcRefCell<T> {
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

    pub fn try_borrow(&self) -> Result<GcRef<'_, T>, core::cell::BorrowError> {
        self.inner.try_borrow().map(GcRef)
    }

    pub fn try_borrow_mut(&self) -> Result<GcRefMut<'_, T>, core::cell::BorrowMutError> {
        self.inner.try_borrow_mut().map(GcRefMut)
    }
}

/// A shared borrow guard returned by [`GcRefCell::borrow`].
pub struct GcRef<'a, T: Trace + ?Sized>(Ref<'a, T>);

impl<T: Trace + ?Sized> Deref for GcRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Trace + core::fmt::Debug + ?Sized> core::fmt::Debug for GcRef<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T: Trace + core::fmt::Display + ?Sized> core::fmt::Display for GcRef<'_, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<'a, T: Trace + ?Sized> GcRef<'a, T> {
    /// Projects a `GcRef<T>` to a `GcRef<U>` via a closure, mirroring
    /// [`Ref::map`].
    pub fn map<U, F>(orig: Self, f: F) -> GcRef<'a, U>
    where
        U: Trace + ?Sized,
        F: FnOnce(&T) -> &U,
    {
        GcRef(Ref::map(orig.0, f))
    }

    pub fn try_map<U: Trace + ?Sized, F>(orig: GcRef<'a, T>, f: F) -> Option<GcRef<'a, U>>
    where
        F: FnOnce(&T) -> Option<&U>,
    {
        Ref::filter_map(orig.0, f).ok().map(GcRef)
    }

    /// Casts a `GcRef<T>` to a `GcRef<U>` without type checking.
    ///
    /// # Safety
    ///
    /// `T` and `U` must be pointer-compatible. The underlying value must be
    /// a valid `U`.
    pub unsafe fn cast<U: Trace>(orig: GcRef<'a, T>) -> GcRef<'a, U> {
        GcRef(Ref::map(orig.0, |t| unsafe {
            &*((t as *const T).cast::<U>())
        }))
    }
}

/// A mutable borrow guard returned by [`GcRefCell::borrow_mut`].
pub struct GcRefMut<'a, T: Trace + ?Sized>(RefMut<'a, T>);

impl<T: Trace + ?Sized> Deref for GcRefMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Trace + ?Sized> DerefMut for GcRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<'a, T: Trace + ?Sized> GcRefMut<'a, T> {
    /// Projects a `GcRefMut<T>` to a `GcRefMut<U>` via a closure, mirroring
    /// [`RefMut::map`].
    pub fn map<U, F>(orig: Self, f: F) -> GcRefMut<'a, U>
    where
        U: Trace + ?Sized,
        F: FnOnce(&mut T) -> &mut U,
    {
        GcRefMut(RefMut::map(orig.0, f))
    }

    pub fn try_map<U: Trace + ?Sized, F>(orig: GcRefMut<'a, T>, f: F) -> Option<GcRefMut<'a, U>>
    where
        F: FnOnce(&mut T) -> Option<&mut U>,
    {
        RefMut::filter_map(orig.0, f).ok().map(GcRefMut)
    }

    /// Casts a `GcRefMut<T>` to a `GcRefMut<U>` without type checking.
    ///
    /// # Safety
    ///
    /// `T` and `U` must be pointer compatible, the underlying value must be
    /// a valid `U`
    pub unsafe fn cast<U: Trace>(orig: GcRefMut<'a, T>) -> GcRefMut<'a, U> {
        GcRefMut(RefMut::map(orig.0, |t| unsafe {
            &mut *((t as *mut T).cast::<U>())
        }))
    }
}

impl<T: Trace + ?Sized> Finalize for GcRefCell<T> {}

unsafe impl<T: Trace + ?Sized> Trace for GcRefCell<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        let val = unsafe { &*self.inner.as_ptr() };
        unsafe {
            val.trace(tracer);
        }
    }
}
