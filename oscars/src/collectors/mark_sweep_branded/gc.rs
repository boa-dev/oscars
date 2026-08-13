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
use typeid;

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

impl<'gc, T: Trace + Finalize + 'gc> Gc<'gc, T> {
    /// Allocates `value` on the GC heap via the provided `MutationContext`.
    ///
    /// This is a convenience wrapper over [`crate::collectors::mark_sweep_branded::MutationContext::try_alloc`] that
    /// panics on allocation failure. It mirrors the `null_collector_branded`
    /// `Gc::new` so engine call-sites remain unchanged under `mark_sweep_backend`.
    #[inline]
    pub fn new(
        mc: &crate::collectors::mark_sweep_branded::MutationContext<'_, 'gc>,
        value: T,
    ) -> Self {
        mc.try_alloc(value)
            .expect("mark_sweep_branded: Gc::new allocation failed")
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Gc<'gc, T> {
    /// Returns a shared reference to the value.
    #[inline]
    pub(crate) fn inner_ref(&self) -> &T {
        // SAFETY: `ptr` is non-null and valid for `'gc` by construction.
        // The `'gc` lifetime is scoped to a `mutate()` closure, collection only occurs
        // via `cx.collect()` within that same closure and `Gc<'gc, T>` can't
        // escape the closure.
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
            _marker: PhantomData,
        }
    }

    /// Returns `true` if the inner value is of type `U`.
    ///
    /// Uses `typeid::of::<U>()`, sound even when `U` carries a branded
    /// lifetime because it properly handles branded lifetimes. This avoids the `T: 'static` restriction while still
    /// giving us a stable, unique identity guarantee.
    #[inline]
    pub fn is<U: Trace + ?Sized>(&self) -> bool {
        let actual_type_id = unsafe { (*self.ptr.as_ptr().as_ptr()).0.type_id };
        actual_type_id == typeid::of::<U>()
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
            _marker: PhantomData,
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
        crate::collectors::mark_sweep_branded::MutationContext::global()
            .try_alloc(Default::default())
            .unwrap()
    }
}

impl<'gc, T: Trace + fmt::Display + ?Sized + 'gc> fmt::Display for Gc<'gc, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.inner_ref(), f)
    }
}

impl<'gc, T: Trace + ?Sized + 'gc> Deref for Gc<'gc, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.inner_ref()
    }
}

impl<T: Trace + ?Sized> Finalize for Gc<'_, T> {}
unsafe impl<'gc, T: Trace + ?Sized + 'gc> Trace for Gc<'gc, T> {
    unsafe fn trace(&self, tracer: &mut crate::collectors::mark_sweep_branded::trace::Tracer) {
        tracer.mark(self);
    }
}
