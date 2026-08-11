//! Core pointer types.

use crate::{
    alloc::mempool3::PoolPointer,
    collectors::null_collector_branded::{
        gc_box::GcBox,
        trace::{Finalize, Trace},
    },
};
use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;

/// Transient pointer to a GC managed value.
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

    pub fn new<'id>(
        mc: &crate::collectors::null_collector_branded::MutationContext<'id, 'gc>,
        value: T,
    ) -> Self
    where
        T: Sized + Finalize,
    {
        mc.try_alloc(value).unwrap()
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Gc<'gc, T> {
    /// Returns a shared reference to the value.
    #[inline]
    pub(crate) fn inner_ref(&self) -> &T {
        // SAFETY: `ptr` is non-null and valid for `'gc` by construction.
        unsafe { &(*self.ptr.as_ptr().as_ptr()).0.value }
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.inner_ref() as *const T
    }

    #[inline]
    #[must_use]
    pub fn ptr_eq<U: Trace + ?Sized + 'gc>(this: &Self, other: &Gc<'gc, U>) -> bool {
        core::ptr::eq(this.as_ptr() as *const (), other.as_ptr() as *const ())
    }

    /// Casts the internal pointer to a different type.
    ///
    /// # Safety
    /// The caller must ensure that the inner value is valid for the target type `U`.
    #[inline]
    pub unsafe fn cast_unchecked<U: Trace + 'gc>(self) -> Gc<'gc, U> {
        let raw = self
            .ptr
            .as_ptr()
            .cast::<crate::alloc::mempool3::PoolItem<GcBox<U>>>();
        Gc {
            ptr: unsafe { crate::alloc::mempool3::PoolPointer::from_raw(raw) },
            _marker: core::marker::PhantomData,
        }
    }

    #[inline]
    pub fn is<U: Trace + ?Sized + 'gc>(&self) -> bool {
        let actual_type_name = unsafe { (*self.ptr.as_ptr().as_ptr()).0.type_name };
        actual_type_name == core::any::type_name::<U>()
    }

    #[inline]
    #[allow(private_interfaces)]
    pub fn into_raw(self) -> core::ptr::NonNull<crate::alloc::mempool3::PoolItem<GcBox<T>>> {
        let ptr = self.ptr.as_ptr();
        let _ = self;
        ptr
    }

    /// Constructs a `Gc` from a raw pointer.
    ///
    /// # Safety
    /// The pointer must have been previously obtained from `into_raw`.
    #[inline]
    #[allow(private_interfaces)]
    pub unsafe fn from_raw(
        ptr: core::ptr::NonNull<crate::alloc::mempool3::PoolItem<GcBox<T>>>,
    ) -> Self {
        Self {
            ptr: unsafe { crate::alloc::mempool3::PoolPointer::from_raw(ptr) },
            _marker: core::marker::PhantomData,
        }
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> AsRef<T> for Gc<'gc, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self.inner_ref()
    }
}

#[cfg(feature = "std")]
impl<'gc, T: Trace + Finalize + Default + 'gc> Default for Gc<'gc, T> {
    fn default() -> Self {
        crate::collectors::null_collector_branded::MutationContext::global()
            .try_alloc(Default::default())
            .unwrap()
    }
}
impl<'gc, T: Trace + ?Sized + fmt::Display + 'gc> fmt::Display for Gc<'gc, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.inner_ref(), f)
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Deref for Gc<'gc, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.inner_ref()
    }
}

impl<T: Trace + ?Sized> Finalize for Gc<'_, T> {}

unsafe impl<T: Trace + ?Sized> Trace for Gc<'_, T> {
    unsafe fn trace(&self, tracer: &mut crate::collectors::null_collector_branded::trace::Tracer) {
        tracer.mark(self);
    }
}
