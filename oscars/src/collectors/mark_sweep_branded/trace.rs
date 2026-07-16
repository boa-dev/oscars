//! Trace and Finalize traits for the lifetime branded GC

#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    alloc::mempool3::PoolItem,
    collectors::mark_sweep_branded::{gc::Gc, gc_box::GcColor},
};
use core::cell::{Cell, OnceCell};
use core::marker::PhantomData;
use rust_alloc::borrow::{Cow, ToOwned};
use rust_alloc::boxed::Box;
use rust_alloc::collections::{BTreeMap, BTreeSet, LinkedList, VecDeque};
use rust_alloc::string::String;
use rust_alloc::vec::Vec;

// Re-export the shared `Finalize` trait and standard library implementations.
pub use crate::collectors::common::Finalize;

/// Trait for tracing garbage collected values.
///
/// # Safety
///
/// Use `Tracer::mark` for every reachable `Gc` pointer.
pub unsafe trait Trace {
    /// Marks all `Gc` pointers reachable from `self`.
    ///
    /// # Safety
    ///
    /// See `boa_gc::Trace` for safety contract details.
    unsafe fn trace(&self, tracer: &mut Tracer);
}

pub(crate) type TraceFn = unsafe fn(core::ptr::NonNull<u8>, &mut Tracer<'_>);

/// Worklist-driven mark context for a stop-the-world collection cycle.
///
/// Implements the classic tri-color marking invariant
/// (see `GcColor` for the per-object states):
///
/// - `mark()` transitions `White → Gray` and enqueues the object.
/// - `drain()` dequeues each Gray entry; `gc_box::trace_value` transitions
///   it `Gray → Black` and recurses into its children.
/// - The sweep phase reclaims all remaining White objects and resets
///   Black → White, restoring the invariant for the next cycle.
///
/// The worklist provides iterative traversal, preventing stack overflow on
/// deeply nested object graphs.
///
/// The `'a` lifetime ties the tracer to the collection cycle,
/// preventing it from being stored or escaping the collector.
pub struct Tracer<'a> {
    pub(crate) worklist: Vec<(core::ptr::NonNull<u8>, TraceFn)>,
    pub(crate) _marker: PhantomData<&'a ()>,
}

impl<'a> Tracer<'a> {
    pub(crate) fn new() -> Self {
        Self {
            worklist: Vec::new(),
            _marker: PhantomData,
        }
    }

    pub(crate) fn drain(&mut self) {
        // Note: Using `pop()` processes the worklist in LIFO order (Depth-First Search).
        // While correct, heap-allocated object graphs often exhibit better cache locality
        // with Breadth-First Search. This could be evaluated with a `VecDeque` in the future.
        while let Some((ptr, trace_fn)) = self.worklist.pop() {
            // SAFETY: ptr is a live PoolItem<GcBox<T>> whose TraceFn was stored at allocation.
            // pop() releases the borrow on self.worklist before the call, allowing mark()
            // to push new entries re-entrantly.
            unsafe { (trace_fn)(ptr, self) }
        }
    }

    /// Marks `gc` as reachable (White → Gray).
    #[inline]
    pub fn mark<T: Trace>(&mut self, gc: &Gc<'_, T>) {
        // SAFETY: `gc.ptr` is a valid `PoolItem<GcBox<T>>`.
        unsafe {
            let gc_box = &(*gc.ptr.as_ptr().as_ptr()).0;
            if gc_box.color.get() == GcColor::White {
                gc_box.color.set(GcColor::Gray);
                self.worklist.push((
                    gc.ptr.as_ptr().cast::<u8>(),
                    crate::collectors::mark_sweep_branded::gc_box::trace_value::<T>,
                ));
            }
        }
    }

    /// Marks a raw allocation as reachable, returning `true` if newly marked.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid pointer to a `PoolItem<GcBox<_>>` managed by this collector.
    #[inline]
    pub(crate) fn mark_raw(&mut self, ptr: core::ptr::NonNull<u8>) -> bool {
        let pool_item_ptr =
            ptr.cast::<PoolItem<crate::collectors::mark_sweep_branded::gc_box::GcBox<()>>>();

        unsafe {
            let gc_box = &(*pool_item_ptr.as_ptr()).0;
            if gc_box.color.get() == GcColor::White {
                let trace_fn = gc_box.trace_fn;
                gc_box.color.set(GcColor::Gray);
                self.worklist.push((ptr, trace_fn));
                true
            } else {
                false
            }
        }
    }
}

unsafe impl<T: ?Sized> Trace for &T {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// primitive + std-lib Trace impls

macro_rules! empty_trace {
    ($($T:ty),* $(,)?) => {
        $(
            unsafe impl Trace for $T {
                #[inline]
                unsafe fn trace(&self, _tracer: &mut Tracer) {}
            }
        )*
    };
}

empty_trace![
    (),
    bool,
    isize,
    usize,
    i8,
    u8,
    i16,
    u16,
    i32,
    u32,
    i64,
    u64,
    i128,
    u128,
    f32,
    f64,
    char,
    String,
    core::num::NonZeroIsize,
    core::num::NonZeroUsize,
    core::num::NonZeroI8,
    core::num::NonZeroU8,
    core::num::NonZeroI16,
    core::num::NonZeroU16,
    core::num::NonZeroI32,
    core::num::NonZeroU32,
    core::num::NonZeroI64,
    core::num::NonZeroU64,
    core::num::NonZeroI128,
    core::num::NonZeroU128,
];

unsafe impl<T: Trace, const N: usize> Trace for [T; N] {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace> Trace for Box<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        (**self).trace(tracer);
    }
}

unsafe impl<T: Trace> Trace for Option<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Some(v) = self {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace, E: Trace> Trace for Result<T, E> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        match self {
            Ok(v) => v.trace(tracer),
            Err(e) => e.trace(tracer),
        }
    }
}

unsafe impl<T: Trace> Trace for Vec<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace> Trace for VecDeque<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace> Trace for LinkedList<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T> Trace for PhantomData<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// Cell<Option<T>> requires T: Copy to safely read the value via Cell::get().
// For non-Copy types, use GcRefCell instead.
unsafe impl<T: Trace + Default> Trace for Cell<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        let v = self.take();
        v.trace(tracer);
        self.set(v);
    }
}

unsafe impl<T: Trace> Trace for OnceCell<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Some(v) = self.get() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: ToOwned + Trace + ?Sized> Trace for Cow<'static, T>
where
    T::Owned: Trace,
{
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Cow::Owned(v) = self {
            v.trace(tracer);
        }
    }
}

unsafe impl<A: Trace> Trace for (A,) {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace> Trace for (A, B) {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace> Trace for (A, B, C) {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace> Trace for (A, B, C, D) {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
    }
}

// Rc and Arc do not contain Gc pointers (they use reference counting, not GC).
// If you need to store Gc pointers inside Rc/Arc, wrap them in a GC-allocated
// struct instead.
unsafe impl<T: ?Sized> Trace for rust_alloc::rc::Rc<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

unsafe impl<T: ?Sized> Trace for rust_alloc::sync::Arc<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

unsafe impl<K, V: Trace> Trace for BTreeMap<K, V> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.values() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T> Trace for BTreeSet<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {
        // BTreeSet keys are immutable and cannot contain Gc pointers
        // that need tracing (Gc requires &mut self to trace).
    }
}
