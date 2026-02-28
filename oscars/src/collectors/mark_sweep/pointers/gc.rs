use crate::alloc::arena2::{ArenaHeapItem, ArenaPointer, ErasedArenaPointer};
use crate::collectors::collector::Collector;
use crate::collectors::mark_sweep::Finalize;
use crate::collectors::mark_sweep::internals::NonTraceable;
use crate::collectors::mark_sweep::{internals::GcBox, trace::Trace};
use core::any::TypeId;
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display};
use core::ops::Deref;
use core::{marker::PhantomData, ptr::NonNull};

/// A garbage-collected handle that acts as an external root
pub struct Root<T: Trace + ?Sized + 'static> {
    pub(crate) inner_ptr: ErasedArenaPointer<'static>,
    pub(crate) marker: PhantomData<T>,
}

/// A garbage-collected pointer for use as internal struct fields.
pub struct Gc<T: Trace + ?Sized + 'static> {
    pub(crate) inner_ptr: ErasedArenaPointer<'static>,
    pub(crate) marker: PhantomData<T>,
}

impl<T: Trace> Root<T> {
    #[must_use]
    pub fn new_in<C: Collector>(value: T, collector: &C) -> Self {
        let inner_ptr = collector
            .alloc_gc_node(value)
            .expect("Failed to allocate Gc node")
            .to_erased();

        let root = Self {
            inner_ptr,
            marker: PhantomData,
        };
        // The GcBox is allocated with 0 roots by default, Root takes ownership of 1 root
        root.inner_ptr().as_inner_ref().inc_roots();
        root
    }
}

macro_rules! ptr_impls_sized {
    ($name:ident) => {
        impl<T: Trace> $name<T> {
            pub(crate) fn inner_ptr(&self) -> ArenaPointer<'static, GcBox<T>> {
                unsafe { self.inner_ptr.to_typed_arena_pointer::<GcBox<T>>() }
            }
        }
    };
}

macro_rules! ptr_impls_unsized {
    ($name:ident) => {
        impl<T: Trace + ?Sized> $name<T> {
            pub(crate) fn as_sized_inner_ptr(&self) -> NonNull<GcBox<NonTraceable>> {
                let heap_item = unsafe { self.as_heap_ptr().as_mut() };
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

        impl<T: Trace> Deref for $name<T> {
            type Target = T;
            fn deref(&self) -> &T {
                self.inner_ptr().as_inner_ref().value()
            }
        }

        #[allow(clippy::inline_always)]
        impl<T: Trace + PartialEq> PartialEq for $name<T> {
            #[inline(always)]
            fn eq(&self, other: &Self) -> bool {
                **self == **other
            }
        }

        impl<T: Trace + Eq> Eq for $name<T> {}

        #[allow(clippy::inline_always)]
        impl<T: Trace + PartialOrd> PartialOrd for $name<T> {
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

        impl<T: Trace + Ord> Ord for $name<T> {
            fn cmp(&self, other: &Self) -> Ordering {
                (**self).cmp(&**other)
            }
        }

        impl<T: Trace + Display> Display for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                Display::fmt(&**self, f)
            }
        }

        impl<T: Trace + Debug> Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                Debug::fmt(&**self, f)
            }
        }
    };
}

ptr_impls_sized!(Root);
ptr_impls_unsized!(Root);
ptr_impls_sized!(Gc);
ptr_impls_unsized!(Gc);

impl<T: Trace + ?Sized> Root<T> {
    pub fn into_gc(&self) -> Gc<T> {
        Gc {
            inner_ptr: self.inner_ptr,
            marker: PhantomData,
        }
    }
}

impl<T: Trace + ?Sized> Finalize for Root<T> {
    fn finalize(&self) {
        unsafe {
            self.erased_inner_ptr().as_ref().dec_roots();
        };
    }
}

// Root acts as a handle from the stack, so tracing it traces the inner pointer.
unsafe impl<T: Trace + ?Sized> Trace for Root<T> {
    unsafe fn trace(&self, color: crate::collectors::mark_sweep::TraceColor) {
        let trace_fn = unsafe { self.erased_inner_ptr().as_ref().trace_fn() };
        unsafe { trace_fn(self.as_heap_ptr(), color) }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

impl<T: Trace> Clone for Root<T> {
    fn clone(&self) -> Self {
        self.inner_ptr().as_inner_ref().inc_roots();
        Self {
            inner_ptr: self.inner_ptr,
            marker: PhantomData,
        }
    }
}

impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        Finalize::finalize(self);
    }
}

impl<T: Trace + ?Sized> Finalize for Gc<T> {
    fn finalize(&self) {}
}

unsafe impl<T: Trace + ?Sized> Trace for Gc<T> {
    unsafe fn trace(&self, color: crate::collectors::mark_sweep::TraceColor) {
        let trace_fn = unsafe { self.as_sized_inner_ptr().as_ref().trace_fn() };
        unsafe { trace_fn(self.as_heap_ptr(), color) }
    }

    fn run_finalizer(&self) {}
}

impl<T: Trace + ?Sized> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Trace + ?Sized> Copy for Gc<T> {}
