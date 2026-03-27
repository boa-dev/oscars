use crate::alloc::arena2::{ArenaHeapItem, ArenaPointer, ErasedArenaPointer};

use crate::collectors::mark_sweep_arena2::Finalize;
use crate::collectors::mark_sweep_arena2::internals::NonTraceable;
use crate::collectors::mark_sweep_arena2::{Trace, internals::GcBox};
use core::any::TypeId;
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display};
use core::ops::Deref;
use core::{marker::PhantomData, ptr::NonNull};

/// A garbage-collected pointer type over an immutable value.
pub struct Gc<T: Trace + ?Sized + 'static> {
    pub(crate) inner_ptr: ErasedArenaPointer<'static>,
    pub(crate) marker: PhantomData<T>,
}

impl<T: Trace> Gc<T> {
    #[must_use]
    pub fn new_in(
        value: T,
        collector: &crate::collectors::mark_sweep_arena2::MarkSweepGarbageCollector,
    ) -> Self {
        let inner_ptr = collector
            .alloc_gc_node(value)
            .expect("Failed to allocate Gc node")
            .to_erased();

        // SAFETY: safe because the gc tracks this
        let inner_ptr: ErasedArenaPointer<'static> = unsafe { inner_ptr.extend_lifetime() };

        let gc = Self {
            inner_ptr,
            marker: PhantomData,
        };
        // GcBox is allocated with 0 roots, increment to 1 for the new handle
        gc.inner_ptr().as_inner_ref().inc_roots();
        gc
    }

    /// Converts a `Gc` into a raw [`ArenaPointer`].
    pub fn into_raw(this: Self) -> ArenaPointer<'static, GcBox<T>> {
        let ptr = this.inner_ptr();
        core::mem::forget(this);
        ptr
    }

    /// Creates a `Gc` from the provided [`ArenaPointer`].
    ///
    /// # Safety
    ///
    /// Incorrect usage of `from_raw` can lead to use after free.
    pub unsafe fn from_raw(ptr: ArenaPointer<'static, GcBox<T>>) -> Self {
        Self {
            inner_ptr: ptr.to_erased(),
            marker: PhantomData,
        }
    }

    pub fn ptr_eq<U: Trace + ?Sized>(this: &Self, other: &Gc<U>) -> bool {
        this.inner_ptr.as_non_null() == other.inner_ptr.as_non_null()
    }

    pub fn size(&self) -> usize {
        self.inner_ref().size()
    }

    pub fn type_id(&self) -> TypeId {
        self.inner_ref().type_id()
    }

    pub fn is<U: Trace + 'static>(this: &Self) -> bool {
        Self::type_id(this) == TypeId::of::<U>()
    }

    #[inline]
    pub fn downcast<U: Trace + 'static>(this: Self) -> Option<Gc<U>> {
        if !Gc::is::<U>(&this) {
            return None;
        }
        // Safety: We've validated that the type of `this`  is correct above.
        Some(unsafe { Gc::cast_unchecked::<U>(this) })
    }

    /// Cast the `Gc` from `T` to `U`
    ///
    /// # Safety
    ///
    /// Caller must ensure that `U` is valid for `this`.
    #[inline]
    #[must_use]
    pub unsafe fn cast_unchecked<U: Trace + 'static>(this: Self) -> Gc<U> {
        let inner_ptr = this.inner_ptr;
        core::mem::forget(this);
        Gc {
            inner_ptr,
            marker: PhantomData,
        }
    }
}

impl<T: Trace> Gc<T> {
    pub(crate) fn inner_ptr(&self) -> ArenaPointer<'static, GcBox<T>> {
        unsafe { self.inner_ptr.to_typed_arena_pointer::<GcBox<T>>() }
    }
}

impl<T: Trace + ?Sized> Gc<T> {
    pub(crate) fn as_sized_inner_ptr(&self) -> NonNull<GcBox<NonTraceable>> {
        // SAFETY: `&raw mut` prevents creating `&mut` reference into the
        // arena to avoid stacked borrows during Gc tracing
        let heap_ptr = self.as_heap_ptr();
        let value_ptr = ArenaHeapItem::as_value_ptr(heap_ptr);
        unsafe { NonNull::new_unchecked(value_ptr) }
    }

    pub(crate) fn as_heap_ptr(&self) -> NonNull<ArenaHeapItem<GcBox<NonTraceable>>> {
        self.inner_ptr
            .as_non_null()
            .cast::<ArenaHeapItem<GcBox<NonTraceable>>>()
    }

    pub(crate) fn inner_ref(&self) -> &GcBox<NonTraceable> {
        unsafe { self.as_sized_inner_ptr().as_ref() }
    }
}

impl<T: Trace> Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.inner_ptr().as_inner_ref().value()
    }
}

#[allow(clippy::inline_always)]
impl<T: Trace + PartialEq> PartialEq for Gc<T> {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: Trace + Eq> Eq for Gc<T> {}

#[allow(clippy::inline_always)]
impl<T: Trace + PartialOrd> PartialOrd for Gc<T> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
    #[inline(always)]
    fn lt(&self, other: &Self) -> bool {
        **self < **other
    }
    #[inline(always)]
    fn le(&self, other: &Self) -> bool {
        **self <= **other
    }
    #[inline(always)]
    fn gt(&self, other: &Self) -> bool {
        **self > **other
    }
    #[inline(always)]
    fn ge(&self, other: &Self) -> bool {
        **self >= **other
    }
}

impl<T: Trace + Ord> Ord for Gc<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: Trace + Display> Display for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

impl<T: Trace + Debug> Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<T: Trace + ?Sized> Finalize for Gc<T> {
    fn finalize(&self) {
        unsafe {
            self.as_sized_inner_ptr().as_ref().dec_roots();
        }
    }
}

unsafe impl<T: Trace + ?Sized> Trace for Gc<T> {
    unsafe fn trace(&self, color: crate::collectors::mark_sweep_arena2::TraceColor) {
        let trace_fn = unsafe { self.as_sized_inner_ptr().as_ref().trace_fn() };
        unsafe { trace_fn(self.as_heap_ptr(), color) }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

impl<T: Trace> Clone for Gc<T> {
    fn clone(&self) -> Self {
        self.inner_ptr().as_inner_ref().inc_roots();
        Self {
            inner_ptr: self.inner_ptr,
            marker: PhantomData,
        }
    }
}

impl<T: Trace + ?Sized> Drop for Gc<T> {
    fn drop(&mut self) {
        Finalize::finalize(self);
    }
}
