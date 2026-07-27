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
use core::ptr::NonNull;

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

    pub fn into_raw(self) -> *const T {
        let ptr = self.ptr.as_ptr().as_ptr() as *const T;
        let _ = self;
        ptr
    }

    pub unsafe fn from_raw(ptr: *const T) -> Self {
        unsafe {
            Self::with_pointer(crate::alloc::mempool3::PoolPointer::from_raw(
                core::ptr::NonNull::new_unchecked(ptr as *mut _),
            ))
        }
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Gc<'gc, T> {
    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.as_ref() as *const T
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.get() as *const T
    }
}

impl<'gc, T: Trace + ?Sized + fmt::Display + 'gc> fmt::Display for Gc<'gc, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), f)
    }
}

impl<'id, T: Trace + ?Sized> Gc<'id, T> {
    /// Gets a reference to the inner value.
    pub fn as_ref(&self) -> &T {
        unsafe { &self.ptr.as_ptr().as_ref().0.value }
    }

    /// Converts the `Gc` into a `NonNull` pointer.
    pub fn as_non_null(&self) -> core::ptr::NonNull<T> {
        let ptr = self.ptr.as_ptr().as_ptr() as *mut T;
        unsafe { core::ptr::NonNull::new_unchecked(ptr) }
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Deref for Gc<'gc, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.as_ref()
    }
}

impl<T: Trace + ?Sized> Finalize for Gc<'_, T> {}
unsafe impl<T: Trace + ?Sized> Trace for Gc<'_, T> {
    unsafe fn trace(&self, tracer: &mut crate::collectors::null_collector_branded::trace::Tracer) {
        tracer.mark(self);
    }
}

impl<'gc, T: Default + Trace + Finalize + 'gc> Default for Gc<'gc, T> {
    #[inline]
    fn default() -> Self {
        Self::new(
            &unsafe { crate::collectors::null_collector_branded::MutationContext::dummy() },
            Default::default(),
        )
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Gc<'gc, T> {
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        core::ptr::eq(this.ptr.as_ptr().as_ptr(), other.ptr.as_ptr().as_ptr())
    }

    pub unsafe fn cast_unchecked<U: Trace + 'gc>(this: Self) -> Gc<'gc, U> {
        let ptr = this.ptr.as_ptr().as_ptr()
            as *mut crate::alloc::mempool3::PoolItem<
                crate::collectors::null_collector_branded::gc_box::GcBox<U>,
            >;
        let new_ptr = unsafe {
            crate::alloc::mempool3::PoolPointer::from_raw(core::ptr::NonNull::new_unchecked(ptr))
        };
        let _ = this;
        Gc {
            ptr: new_ptr,
            _marker: core::marker::PhantomData,
        }
    }

    pub fn is<U: Trace + 'static>(this: &Self) -> bool {
        // Check if the underlying `GcBox<T>` has type `U`
        // SAFETY: `this.ptr` is a valid, initialized pointer to a `GcBox` for the lifetime of `this`
        unsafe { this.ptr.as_ptr().as_ref().0.type_name == core::any::type_name::<U>() }
    }
}
