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
use crate::alloc::arena2::{ArenaHeapItem, ErasedArenaPointer};
use core::marker::PhantomData;
use core::ptr::NonNull;

#[repr(transparent)]
pub struct WeakGcBox<T: Trace + ?Sized + 'static> {
    pub(crate) inner_ptr: ErasedArenaPointer<'static>,
    pub(crate) marker: PhantomData<T>,
}

impl<T: Trace + Finalize> WeakGcBox<T> {
    pub fn new_in(value: T, color: TraceColor) -> Self {
        Self(GcBox::new_typed_in::<true>(value, color))
    }

    pub(crate) fn inner_ref(&self) -> &GcBox<NonTraceable> {
        // SAFETY: `erased_inner_ptr` returns a valid pointer
        // the pointed-to value lives for at least as long as `self`
        unsafe { self.erased_inner_ptr().as_ref() }
    }

    pub fn is_reachable(&self, color: TraceColor) -> bool {
        self.inner_ref().is_reachable(color)
    }

    pub(crate) fn mark(&self, color: HeaderColor) {
        self.inner_ref().header.mark(color);
    }

    pub(crate) fn set_unmarked(&self, color: TraceColor) {
        self.0.set_unmarked(color);
    }
}

impl<T: Trace> WeakGcBox<T> {
    pub(crate) fn inner_ptr(&self) -> crate::alloc::arena2::ArenaPointer<'static, GcBox<T>> {
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
    // TODO (potentially): Fix alloc to be generic
    // TODO: after weak map integration, change `color: TraceColor` back to `state: &CollectionState`
    // so `WeakMap::insert` can push allocations into the GC queues
    pub(crate) fn new_in(value: T, color: TraceColor) -> Self {
        Self::new_typed_in::<false>(value, color)
    }

    // TODO (nekevss): What is the best function signature here?
    pub(crate) fn new_typed_in<const IS_WEAK: bool>(value: T, color: TraceColor) -> Self {
        // new objects get the current epoch color so they aren't swept immediately
        // the root count starts at 0;,`Root::new_in` increments it to 1
        let header = match color {
            TraceColor::White => GcHeader::new_typed::<true, IS_WEAK>(),
            TraceColor::Black => GcHeader::new_typed::<false, IS_WEAK>(),
        };

        let vtable = vtable_of::<T>();
        Self {
            header,
            vtable,
            value,
        }
    }

    /// This function ensures the GcBox is unmarked by setting it to the opposite
    /// of the collection state.
    pub(crate) fn set_unmarked(&self, color: TraceColor) {
        match color {
            TraceColor::White => self.header.mark(HeaderColor::Black),
            TraceColor::Black => self.header.mark(HeaderColor::White),
        }
    }
}

impl<T: Trace> GcBox<T> {
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
