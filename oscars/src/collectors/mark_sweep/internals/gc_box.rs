//! Implementation of a garbage collected Box

use core::any::TypeId;

use crate::collectors::mark_sweep::Finalize;
use crate::collectors::mark_sweep::internals::gc_header::{GcHeader, HeaderColor};
use crate::collectors::mark_sweep::{Trace, TraceColor};

use super::{DropFn, TraceFn, VTable, vtable_of};

pub struct NonTraceable(());

impl Finalize for NonTraceable {}

unsafe impl Trace for NonTraceable {
    unsafe fn trace(&self, _color: TraceColor) {
        panic!()
    }

    fn run_finalizer(&self) {
        panic!()
    }
}

// TODO: Do we need the vtable on the neo box

// NOTE: This may not be the best idea, but let's find out.
//
use crate::alloc::arena3::{ArenaHeapItem, ErasedArenaPointer};
use core::marker::PhantomData;
use core::ptr::NonNull;

pub struct WeakGcBox<T: Trace + ?Sized + 'static> {
    pub(crate) inner_ptr: ErasedArenaPointer<'static>,
    pub(crate) marker: PhantomData<T>,
}

impl<T: Trace + Finalize + ?Sized> WeakGcBox<T> {
    pub fn new(inner_ptr: ErasedArenaPointer<'static>) -> Self {
        Self {
            inner_ptr,
            marker: PhantomData,
        }
    }

    pub(crate) fn erased_inner_ptr(&self) -> NonNull<GcBox<NonTraceable>> {
        // SAFETY: `as_heap_ptr` returns a valid pointer to
        // `ArenaHeapItem` whose lifetime is tied to the arena
        let heap_item = unsafe { self.as_heap_ptr().as_mut() };
        // SAFETY: We just removed this value from a NonNull
        unsafe { NonNull::new_unchecked(heap_item.as_ptr()) }
    }

    pub(crate) fn as_heap_ptr(&self) -> NonNull<ArenaHeapItem<GcBox<NonTraceable>>> {
        self.inner_ptr
            .as_non_null()
            .cast::<ArenaHeapItem<GcBox<NonTraceable>>>()
    }

    pub(crate) fn inner_ref(&self) -> &GcBox<NonTraceable> {
        // SAFETY: `erased_inner_ptr` returns a valid pointer
        // the pointed-to value lives for at least as long as `self`
        unsafe { self.erased_inner_ptr().as_ref() }
    }

    pub fn is_reachable(&self, color: TraceColor) -> bool {
        self.inner_ref().is_reachable(color)
    }
}

impl<T: Trace> WeakGcBox<T> {
    pub(crate) fn inner_ptr(&self) -> crate::alloc::arena3::ArenaPointer<'static, GcBox<T>> {
        // SAFETY: This pointer started out as a `GcBox<T>`, so it's safe to cast
        // it back, the `PhantomData` guarantees that the type `T` is still correct
        unsafe { self.inner_ptr.to_typed_arena_pointer::<GcBox<T>>() }
    }

    pub fn value(&self) -> &T {
        self.inner_ptr().as_inner_ref().value()
    }
}

impl<T: Trace + ?Sized> Finalize for WeakGcBox<T> {
    #[inline]
    fn finalize(&self) {
        self.inner_ref().finalize()
    }
}

// NOTE: A weak gc box will mark the box, but it will not continue the trace forward.
unsafe impl<T: Trace + ?Sized> Trace for WeakGcBox<T> {
    unsafe fn trace(&self, color: TraceColor) {
        unsafe {
            let trace_fn = self.inner_ref().trace_fn();
            trace_fn(self.as_heap_ptr(), color);
        }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct GcBox<T: Trace + ?Sized + 'static> {
    pub(crate) header: GcHeader,
    vtable: &'static VTable,
    value: T,
}

impl<T: Trace> GcBox<T> {
    // new objects get the opposite of the current live epoch color so they
    // survive the current sweep cycle
    // root_count starts at 0, `Root::new_in` increments it to 1
    pub(crate) fn new_in(value: T, color: TraceColor) -> Self {
        let header = match color {
            TraceColor::White => GcHeader::new_typed::<true>(),
            TraceColor::Black => GcHeader::new_typed::<false>(),
        };
        Self {
            header,
            vtable: vtable_of::<T>(),
            value,
        }
    }
}

impl<T: Trace + ?Sized> GcBox<T> {
    pub fn value(&self) -> &T {
        &self.value
    }

    pub(crate) fn is_reachable(&self, color: TraceColor) -> bool {
        match color {
            TraceColor::Black => self.header.is_black() || self.header.is_grey(),
            TraceColor::White => self.header.is_white() || self.header.is_grey(),
        }
    }

    pub fn roots(&self) -> u16 {
        self.header.roots()
    }

    pub fn inc_roots(&self) {
        self.header.inc_roots();
    }

    pub fn dec_roots(&self) {
        self.header.dec_roots();
    }

    pub(crate) fn is_rooted(&self) -> bool {
        self.header.is_rooted()
    }

    pub fn mark(&self) {
        self.header.mark(HeaderColor::Grey);
    }

    pub(crate) fn trace_fn(&self) -> TraceFn {
        self.vtable.trace_fn()
    }

    pub(crate) fn drop_fn(&self) -> DropFn {
        self.vtable.drop_fn()
    }

    pub(crate) fn size(&self) -> usize {
        self.vtable.size()
    }

    pub(crate) fn type_id(&self) -> TypeId {
        self.vtable.type_id()
    }

    #[inline]
    fn trace_impl(&self, color: TraceColor) {
        match color {
            TraceColor::White if self.header.is_black() => {
                self.header.mark(HeaderColor::Grey);
                unsafe {
                    Trace::trace(&self.value, color);
                }
                // Mark the header once trace is completed.
                self.header.mark(HeaderColor::White);
            }
            TraceColor::Black if self.header.is_white() => {
                self.header.mark(HeaderColor::Grey);
                unsafe {
                    Trace::trace(&self.value, color);
                }
                // Mark the header once trace is completed.
                self.header.mark(HeaderColor::Black);
            }
            // We have a box that's already grey or marked, so we do not
            // need to continue on the trace
            _ => {}
        }
    }
}

impl<T: Trace> Finalize for GcBox<T> {
    fn finalize(&self) {
        self.value.finalize();
    }
}

unsafe impl<T: Trace> Trace for GcBox<T> {
    unsafe fn trace(&self, color: TraceColor) {
        self.trace_impl(color);
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self);
    }
}
