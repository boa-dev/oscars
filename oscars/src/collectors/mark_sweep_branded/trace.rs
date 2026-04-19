//! Trace and Finalize traits for the lifetime branded GC

use crate::collectors::mark_sweep_branded::gc::Gc;
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
pub trait Trace {
    /// Marks all `Gc` pointers reachable from `self`.
    fn trace(&mut self, tracer: &mut Tracer);
}

pub(crate) type TraceFn = unsafe fn(core::ptr::NonNull<u8>, &mut Tracer<'_>);

/// Callback handle passed to `Trace::trace`.
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
            unsafe {
                (trace_fn)(ptr, self);
            }
        }
    }

    /// Marks `gc` as reachable.
    #[inline]
    pub fn mark<T: Trace>(&mut self, gc: &Gc<'_, T>) {
        // SAFETY: `gc.ptr` is a valid `GcBox`.
        unsafe {
            if !(*gc.ptr.as_ptr()).marked.replace(true) {
                unsafe fn trace_value<T: Trace>(
                    ptr: core::ptr::NonNull<u8>,
                    tracer: &mut Tracer<'_>,
                ) {
                    let gcbox_ptr =
                        ptr.cast::<crate::collectors::mark_sweep_branded::gc_box::GcBox<T>>();
                    unsafe {
                        (*gcbox_ptr.as_ptr()).value.trace(tracer);
                    }
                }

                self.worklist.push((gc.ptr.cast::<u8>(), trace_value::<T>));
            }
        }
    }

    /// Marks a raw allocation as reachable.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid pointer to a `GcBox` managed by this collector.
    #[inline]
    pub(crate) fn mark_raw(&mut self, ptr: core::ptr::NonNull<u8>) {
        let boxed_ptr = ptr.cast::<crate::collectors::mark_sweep_branded::gc_box::GcBox<()>>();

        unsafe {
            if !(*boxed_ptr.as_ptr()).marked.replace(true) {
                // Call the trace function.
                if let Some(trace_fn) = (*boxed_ptr.as_ptr()).trace_fn {
                    self.worklist.push((ptr, trace_fn));
                }
            }
        }
    }
}

impl<T: ?Sized> Trace for &T {
    #[inline]
    fn trace(&mut self, _tracer: &mut Tracer) {}
}

// primitive + std-lib Trace impls

macro_rules! empty_trace {
    ($($T:ty),* $(,)?) => {
        $(
            impl Trace for $T {
                #[inline]
                fn trace(&mut self, _tracer: &mut Tracer) {}
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

impl<T: Trace, const N: usize> Trace for [T; N] {
    fn trace(&mut self, tracer: &mut Tracer) {
        for v in self.iter_mut() {
            v.trace(tracer);
        }
    }
}

impl<T: Trace> Trace for Box<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        (**self).trace(tracer);
    }
}

impl<T: Trace> Trace for Option<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        if let Some(v) = self {
            v.trace(tracer);
        }
    }
}

impl<T: Trace, E: Trace> Trace for Result<T, E> {
    fn trace(&mut self, tracer: &mut Tracer) {
        match self {
            Ok(v) => v.trace(tracer),
            Err(e) => e.trace(tracer),
        }
    }
}

impl<T: Trace> Trace for Vec<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        for v in self.iter_mut() {
            v.trace(tracer);
        }
    }
}

impl<T: Trace> Trace for VecDeque<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        for v in self.iter_mut() {
            v.trace(tracer);
        }
    }
}

impl<T: Trace> Trace for LinkedList<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        for v in self.iter_mut() {
            v.trace(tracer);
        }
    }
}

impl<T> Trace for PhantomData<T> {
    #[inline]
    fn trace(&mut self, _tracer: &mut Tracer) {}
}

// Cell<Option<T>> requires T: Copy to safely take and restore the value.
// For non-Copy types, use GcRefCell instead.
impl<T: Copy + Trace> Trace for Cell<Option<T>> {
    fn trace(&mut self, tracer: &mut Tracer) {
        if let Some(mut v) = self.get_mut().take() {
            v.trace(tracer);
            self.set(Some(v));
        }
    }
}

impl<T: Trace> Trace for OnceCell<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        if let Some(v) = self.get_mut() {
            v.trace(tracer);
        }
    }
}

impl<T: ToOwned + Trace + ?Sized> Trace for Cow<'static, T>
where
    T::Owned: Trace,
{
    fn trace(&mut self, tracer: &mut Tracer) {
        if let Cow::Owned(v) = self {
            v.trace(tracer);
        }
    }
}

impl<A: Trace> Trace for (A,) {
    #[inline]
    fn trace(&mut self, tracer: &mut Tracer) {
        self.0.trace(tracer);
    }
}

impl<A: Trace, B: Trace> Trace for (A, B) {
    #[inline]
    fn trace(&mut self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
    }
}

impl<A: Trace, B: Trace, C: Trace> Trace for (A, B, C) {
    #[inline]
    fn trace(&mut self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
    }
}

impl<A: Trace, B: Trace, C: Trace, D: Trace> Trace for (A, B, C, D) {
    #[inline]
    fn trace(&mut self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
    }
}

// Rc and Arc do not contain Gc pointers (they use reference counting, not GC).
// If you need to store Gc pointers inside Rc/Arc, wrap them in a GC-allocated
// struct instead.
impl<T: ?Sized> Trace for rust_alloc::rc::Rc<T> {
    #[inline]
    fn trace(&mut self, _tracer: &mut Tracer) {}
}

impl<T: ?Sized> Trace for rust_alloc::sync::Arc<T> {
    #[inline]
    fn trace(&mut self, _tracer: &mut Tracer) {}
}

impl<K, V: Trace> Trace for BTreeMap<K, V> {
    fn trace(&mut self, tracer: &mut Tracer) {
        for v in self.values_mut() {
            v.trace(tracer);
        }
    }
}

impl<T> Trace for BTreeSet<T> {
    #[inline]
    fn trace(&mut self, _tracer: &mut Tracer) {
        // BTreeSet keys are immutable and cannot contain Gc pointers
        // that need tracing (Gc requires &mut self to trace).
    }
}
