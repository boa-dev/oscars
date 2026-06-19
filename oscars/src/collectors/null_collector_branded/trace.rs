//! Trace and Finalize traits for the lifetime branded GC
pub use crate::collectors::common::Finalize;

use core::cell::{Cell, OnceCell};
use core::marker::PhantomData;
use rust_alloc::borrow::{Cow, ToOwned};
use rust_alloc::boxed::Box;
use rust_alloc::collections::{BTreeMap, BTreeSet, LinkedList, VecDeque};
use rust_alloc::string::String;
use rust_alloc::vec::Vec;

/// Trait for tracing garbage collected values.
///
/// In the null collector, tracing is a no-op.
pub trait Trace {
    /// Marks all Gc pointers reachable from `self`.
    fn trace(&mut self, tracer: &mut Tracer);
}

/// Dummy tracer for the null collector
pub struct Tracer<'a> {
    pub(crate) _marker: PhantomData<&'a ()>,
}

impl<'a> Tracer<'a> {
    #[inline]
    pub fn mark<T: Trace>(
        &mut self,
        _gc: &crate::collectors::null_collector_branded::gc::Gc<'_, T>,
    ) {
    }
}

impl<T: ?Sized> Trace for &T {
    #[inline]
    fn trace(&mut self, _tracer: &mut Tracer) {}
}

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

impl<T: Copy + Trace> Trace for Cell<Option<T>> {
    fn trace(&mut self, tracer: &mut Tracer) {
        if let Some(mut v) = self.get() {
            v.trace(tracer);
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
    fn trace(&mut self, _tracer: &mut Tracer) {}
}
