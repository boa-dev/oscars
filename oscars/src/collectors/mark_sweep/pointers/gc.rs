use crate::alloc::arena2::{ArenaHeapItem, ArenaPointer, ErasedArenaPointer};
use crate::collectors::mark_sweep::internals::NonTraceable;
use crate::collectors::mark_sweep::{Finalize, MarkSweepGarbageCollector};
use crate::collectors::mark_sweep::{internals::GcBox, trace::Trace};
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
    /// Constructs a new `Gc<T>` with the given value.
    #[must_use]
    pub fn new_in(value: T, collector: &mut MarkSweepGarbageCollector) -> Self {
        // Create GcBox
        let gc_box = GcBox::new_in(value, &collector.state);
        let inner_ptr = collector.alloc_with_collection(gc_box).to_erased();

        Self {
            inner_ptr,
            marker: PhantomData,
        }
    }

    pub(crate) fn inner_ptr(&self) -> ArenaPointer<'static, GcBox<T>> {
        unsafe { self.inner_ptr.to_typed_arena_pointer::<GcBox<T>>() }
    }
}

impl<T: Trace + ?Sized> Gc<T> {
    pub(crate) fn as_sized_inner_ptr(&self) -> NonNull<GcBox<NonTraceable>> {
        let heap_item = unsafe { self.as_heap_ptr().as_mut() };
        // SAFETY: We just removed this value from a NonNull.
        unsafe { NonNull::new_unchecked(heap_item.as_ptr()) }
    }

    pub(crate) fn as_heap_ptr(&self) -> NonNull<ArenaHeapItem<GcBox<NonTraceable>>> {
        self.inner_ptr
            .as_non_null()
            .cast::<ArenaHeapItem<GcBox<NonTraceable>>>()
    }

    pub(crate) fn inner_ref(&self) -> &GcBox<NonTraceable> {
        unsafe { self.as_sized_inner_ptr().as_ref() }
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
}

impl<T: Trace + ?Sized> Finalize for Gc<T> {
    fn finalize(&self) {
        unsafe {
            self.as_sized_inner_ptr().as_ref().dec_roots();
        };
    }
}

unsafe impl<T: Trace + ?Sized> Trace for Gc<T> {
    unsafe fn trace(&self, color: crate::collectors::mark_sweep::TraceColor) {
        let trace_fn = unsafe { self.as_sized_inner_ptr().as_ref().trace_fn() };
        unsafe { trace_fn(self.as_heap_ptr(), color) }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

impl<T: Trace> Clone for Gc<T> {
    fn clone(&self) -> Self {
        // Increment root count and copy pointer
        self.inner_ptr().as_inner_ref().inc_roots();
        Self {
            inner_ptr: self.inner_ptr,
            marker: PhantomData,
        }
    }
}

impl<T: Trace + ?Sized> Drop for Gc<T> {
    fn drop(&mut self) {
        // SAFETY: the pointer should be valid for a reference.
        Finalize::finalize(self);
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
