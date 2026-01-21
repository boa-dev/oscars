//! Implementation of a garbage collected Box

use core::any::TypeId;

use crate::collectors::mark_sweep::internals::gc_header::{GcHeader, HeaderColor};
use crate::collectors::mark_sweep::{CollectionState, Finalize};
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
// If we spend too much time relying on this type, then we
// may be able to remove the weak flag from GcHeader
#[repr(transparent)]
pub struct WeakGcBox<T: Trace + ?Sized + 'static>(GcBox<T>);

impl<T: Trace + Finalize> WeakGcBox<T> {
    pub fn new_in(value: T, collection_state: &CollectionState) -> Self {
        Self(GcBox::new_typed_in::<true>(value, collection_state))
    }

    pub fn value(&self) -> &T {
        self.0.value()
    }

    pub fn is_reachable(&self, color: TraceColor) -> bool {
        self.0.is_reachable(color)
    }

    pub(crate) fn mark(&self, color: HeaderColor) {
        self.0.header.mark(color);
    }

    pub(crate) fn set_unmarked(&self, state: &CollectionState) {
        self.0.set_unmarked(state);
    }
}

impl<T: Trace> Finalize for WeakGcBox<T> {
    #[inline]
    fn finalize(&self) {
        self.0.finalize()
    }
}

// NOTE: A weak gc box will mark the box, but it will not continue the trace forward.
unsafe impl<T: Trace> Trace for WeakGcBox<T> {
    unsafe fn trace(&self, color: TraceColor) {
        unsafe {
            self.0.trace(color);
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
    pub(crate) fn new_in(value: T, collection_state: &CollectionState) -> Self {
        Self::new_typed_in::<false>(value, collection_state)
    }

    // TODO (nekevss): What is the best function signature here?
    pub(crate) fn new_typed_in<const IS_WEAK: bool>(
        value: T,
        collection_state: &CollectionState,
    ) -> Self {
        extern crate std;
        let header = match collection_state.color {
            TraceColor::White => GcHeader::new_typed::<true, IS_WEAK>(),
            TraceColor::Black => GcHeader::new_typed::<false, IS_WEAK>(),
        };
        // Increment the root for this box.
        header.inc_roots();

        let vtable = vtable_of::<T>();
        Self {
            header,
            vtable,
            value,
        }
    }

    /// This function ensures the GcBox is unmarked by setting it to the opposite
    /// of the collection state.
    pub(crate) fn set_unmarked(&self, state: &CollectionState) {
        match state.color {
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
            // If the GcBox is marked as weak, and it is grey,
            // then that means that we need to actually trace
            // it's contents and mark it as alive.
            //
            // We are safe to do this, because Ephemeron prevents
            // us from accessing the WeakGcBox early.
            _ if self.header.is_weak() & self.header.is_grey() => {
                unsafe {
                    Trace::trace(&self.value, color);
                }
                let color = match color {
                    TraceColor::Black => HeaderColor::Black,
                    TraceColor::White => HeaderColor::White,
                };
                self.header.mark(color);
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
