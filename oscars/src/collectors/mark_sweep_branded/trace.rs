//! Trace and Finalize traits for the lifetime branded GC

use crate::{alloc::mempool3::PoolItem, collectors::mark_sweep_branded::gc::Gc};
use core::cell::{Cell, OnceCell, RefCell};
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
/// Use `TraceColor::mark` for every reachable `Gc` pointer.
pub trait Trace {
    /// Marks all `Gc` pointers reachable from `self`.
    fn trace(&self, color: &TraceColor);
}

pub(crate) type TraceFn = unsafe fn(core::ptr::NonNull<u8>, &TraceColor<'_>);

/// Opaque token threaded through a collection cycle.
///
/// The `'a` lifetime ties the color to the collection cycle,
/// preventing it from being stored or escaping the collector.
pub struct TraceColor<'a> {
    pub(crate) worklist: RefCell<Vec<(core::ptr::NonNull<u8>, TraceFn)>>,
    pub(crate) _marker: PhantomData<&'a ()>,
}

impl<'a> TraceColor<'a> {
    pub(crate) fn new() -> Self {
        Self {
            worklist: RefCell::new(Vec::new()),
            _marker: PhantomData,
        }
    }

    pub(crate) fn drain(&self) {
        // Note: Using `pop()` processes the worklist in LIFO order (Depth-First Search).
        // While correct, heap-allocated object graphs often exhibit better cache locality
        // with Breadth-First Search. This could be evaluated with a `VecDeque` in the future.
        loop {
            // Drop the borrow before calling trace_fn to allow re-entrant marks.
            let item = self.worklist.borrow_mut().pop();
            match item {
                Some((ptr, trace_fn)) => unsafe { (trace_fn)(ptr, self) },
                None => break,
            }
        }
    }

    /// Marks `gc` as reachable.
    #[inline]
    pub fn mark<T: Trace>(&self, gc: &Gc<'_, T>) {
        // SAFETY: `gc.ptr` is a valid `PoolItem<GcBox<T>>`.
        unsafe {
            if !(*gc.ptr.as_ptr()).0.marked.replace(true) {
                unsafe fn trace_value<T: Trace>(
                    ptr: core::ptr::NonNull<u8>,
                    color: &TraceColor<'_>,
                ) {
                    let pool_item_ptr = ptr
                        .cast::<PoolItem<crate::collectors::mark_sweep_branded::gc_box::GcBox<T>>>(
                        );
                    unsafe {
                        (*pool_item_ptr.as_ptr()).0.value.trace(color);
                    }
                }

                self.worklist
                    .borrow_mut()
                    .push((gc.ptr.cast::<u8>(), trace_value::<T>));
            }
        }
    }

    /// Marks a raw allocation as reachable, returning `true` if newly marked.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid pointer to a `PoolItem<GcBox<_>>` managed by this collector.
    #[inline]
    pub(crate) fn mark_raw(&self, ptr: core::ptr::NonNull<u8>) -> bool {
        let pool_item_ptr =
            ptr.cast::<PoolItem<crate::collectors::mark_sweep_branded::gc_box::GcBox<()>>>();

        unsafe {
            if !(*pool_item_ptr.as_ptr()).0.marked.replace(true) {
                let trace_fn = (*pool_item_ptr.as_ptr()).0.trace_fn;
                self.worklist.borrow_mut().push((ptr, trace_fn));
                true
            } else {
                false
            }
        }
    }
}

impl<T: ?Sized> Trace for &T {
    #[inline]
    fn trace(&self, _color: &TraceColor) {}
}

// primitive + std-lib Trace impls

macro_rules! empty_trace {
    ($($T:ty),* $(,)?) => {
        $(
            impl Trace for $T {
                #[inline]
                fn trace(&self, _color: &TraceColor) {}
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
    fn trace(&self, color: &TraceColor) {
        for v in self.iter() {
            v.trace(color);
        }
    }
}

impl<T: Trace> Trace for Box<T> {
    fn trace(&self, color: &TraceColor) {
        (**self).trace(color);
    }
}

impl<T: Trace> Trace for Option<T> {
    fn trace(&self, color: &TraceColor) {
        if let Some(v) = self {
            v.trace(color);
        }
    }
}

impl<T: Trace, E: Trace> Trace for Result<T, E> {
    fn trace(&self, color: &TraceColor) {
        match self {
            Ok(v) => v.trace(color),
            Err(e) => e.trace(color),
        }
    }
}

impl<T: Trace> Trace for Vec<T> {
    fn trace(&self, color: &TraceColor) {
        for v in self.iter() {
            v.trace(color);
        }
    }
}

impl<T: Trace> Trace for VecDeque<T> {
    fn trace(&self, color: &TraceColor) {
        for v in self.iter() {
            v.trace(color);
        }
    }
}

impl<T: Trace> Trace for LinkedList<T> {
    fn trace(&self, color: &TraceColor) {
        for v in self.iter() {
            v.trace(color);
        }
    }
}

impl<T> Trace for PhantomData<T> {
    #[inline]
    fn trace(&self, _color: &TraceColor) {}
}

// Cell<Option<T>> requires T: Copy to safely read the value via Cell::get().
// For non-Copy types, use GcRefCell instead.
impl<T: Copy + Trace> Trace for Cell<Option<T>> {
    fn trace(&self, color: &TraceColor) {
        if let Some(v) = self.get() {
            v.trace(color);
        }
    }
}

impl<T: Trace> Trace for OnceCell<T> {
    fn trace(&self, color: &TraceColor) {
        if let Some(v) = self.get() {
            v.trace(color);
        }
    }
}

impl<T: ToOwned + Trace + ?Sized> Trace for Cow<'static, T>
where
    T::Owned: Trace,
{
    fn trace(&self, color: &TraceColor) {
        if let Cow::Owned(v) = self {
            v.trace(color);
        }
    }
}

impl<A: Trace> Trace for (A,) {
    #[inline]
    fn trace(&self, color: &TraceColor) {
        self.0.trace(color);
    }
}

impl<A: Trace, B: Trace> Trace for (A, B) {
    #[inline]
    fn trace(&self, color: &TraceColor) {
        self.0.trace(color);
        self.1.trace(color);
    }
}

impl<A: Trace, B: Trace, C: Trace> Trace for (A, B, C) {
    #[inline]
    fn trace(&self, color: &TraceColor) {
        self.0.trace(color);
        self.1.trace(color);
        self.2.trace(color);
    }
}

impl<A: Trace, B: Trace, C: Trace, D: Trace> Trace for (A, B, C, D) {
    #[inline]
    fn trace(&self, color: &TraceColor) {
        self.0.trace(color);
        self.1.trace(color);
        self.2.trace(color);
        self.3.trace(color);
    }
}

// Rc and Arc do not contain Gc pointers (they use reference counting, not GC).
// If you need to store Gc pointers inside Rc/Arc, wrap them in a GC-allocated
// struct instead.
impl<T: ?Sized> Trace for rust_alloc::rc::Rc<T> {
    #[inline]
    fn trace(&self, _color: &TraceColor) {}
}

impl<T: ?Sized> Trace for rust_alloc::sync::Arc<T> {
    #[inline]
    fn trace(&self, _color: &TraceColor) {}
}

impl<K, V: Trace> Trace for BTreeMap<K, V> {
    fn trace(&self, color: &TraceColor) {
        for v in self.values() {
            v.trace(color);
        }
    }
}

impl<T> Trace for BTreeSet<T> {
    #[inline]
    fn trace(&self, _color: &TraceColor) {
        // BTreeSet keys are immutable and cannot contain Gc pointers
        // that need tracing (Gc requires &self to trace).
    }
}
