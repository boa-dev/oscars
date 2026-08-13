use crate::collectors::null_collector_branded::trace::{Finalize, Trace, Tracer};
use core::cell::{Ref, RefCell, RefMut};
use core::ops::{Deref, DerefMut};

/// GC aware wrapper around [`RefCell<T>`]
pub struct GcRefCell<T: Trace + ?Sized> {
    inner: RefCell<T>,
}

impl<T: Trace> GcRefCell<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
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

/// Shared borrow guard returned by [`GcRefCell::borrow`]
pub struct GcRef<'a, T: Trace + ?Sized>(Ref<'a, T>);

impl<T: Trace + ?Sized> Deref for GcRef<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A mutable borrow guard returned by [`GcRefCell::borrow_mut`]
pub struct GcRefMut<'a, T: Trace + ?Sized>(RefMut<'a, T>);

impl<T: Trace + ?Sized> Deref for GcRefMut<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Trace + ?Sized> DerefMut for GcRefMut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, T: Trace + ?Sized> Clone for GcRef<'a, T> {
    #[inline]
    fn clone(&self) -> Self {
        GcRef(Ref::clone(&self.0))
    }
}

impl<'a, T: Trace + ?Sized> GcRef<'a, T> {
    pub fn map<U: Trace + ?Sized, F>(orig: GcRef<'a, T>, f: F) -> GcRef<'a, U>
    where
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

impl<'a, T: Trace + ?Sized> GcRefMut<'a, T> {
    pub fn map<U: Trace + ?Sized, F>(orig: GcRefMut<'a, T>, f: F) -> GcRefMut<'a, U>
    where
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
    // GcRefCell<'gc, T> is branded by T's lifetime. Map to the static form.
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        // SAFETY: We only access the inner value for tracing and do not mutate it.
        // The null collector's trace is a no-op, so this is safe.
        let val = unsafe { &*self.inner.as_ptr() };
        unsafe {
            val.trace(tracer);
        }
    }
}

impl<T: Trace + Default> Default for GcRefCell<T> {
    #[inline]
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Trace + core::fmt::Debug> core::fmt::Debug for GcRefCell<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.inner.try_borrow() {
            Ok(borrow) => f.debug_tuple("GcRefCell").field(&*borrow).finish(),
            Err(_) => {
                struct BorrowedPlaceholder;
                impl core::fmt::Debug for BorrowedPlaceholder {
                    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                        f.write_str("<borrowed>")
                    }
                }
                f.debug_tuple("GcRefCell")
                    .field(&BorrowedPlaceholder)
                    .finish()
            }
        }
    }
}

impl<T: Trace + Clone> Clone for GcRefCell<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self::new(self.inner.borrow().clone())
    }
}

impl<'a, T: Trace + core::fmt::Debug + ?Sized> core::fmt::Debug for GcRef<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: Trace + core::fmt::Display + ?Sized> core::fmt::Display for GcRef<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

impl<'a, T: Trace + core::fmt::Debug + ?Sized> core::fmt::Debug for GcRefMut<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: Trace + core::fmt::Display + ?Sized> core::fmt::Display for GcRefMut<'a, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}
