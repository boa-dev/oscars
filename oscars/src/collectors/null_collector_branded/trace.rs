//! Trace and Finalize traits for the lifetime branded GC

#![allow(unsafe_op_in_unsafe_fn)]
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
///
/// # Safety
///
/// Implementors must pass every reachable `Gc` pointer to `Tracer::mark`
/// While the null collector reclaims no memory, implementations must be
/// sound for other collectors to prevent UAF bugs.
pub unsafe trait Trace {
    /// Marks all `Gc` pointers reachable from `self`.
    ///
    /// # Safety
    ///
    /// Must only be called by the garbage collector. Implementors must call
    /// `Tracer::mark` on all reachable `Gc` fields and avoid other unsafe operations.
    unsafe fn trace(&self, tracer: &mut Tracer);

    /// Unroots handles located in the GC heap.
    ///
    /// # Safety
    ///
    /// Must only be called by the garbage collector.
    // TODO: remove in the future
    #[inline]
    unsafe fn trace_non_roots(&self) {}

    // TODO: remove in the future
    #[inline]
    fn run_finalizer(&self) {}
}

/// Dummy tracer for the null collector
pub struct Tracer<'a> {
    pub(crate) _marker: PhantomData<&'a ()>,
}

impl<'a> Tracer<'a> {
    #[inline]
    pub fn mark<T: Trace + ?Sized>(
        &mut self,
        _gc: &crate::collectors::null_collector_branded::gc::Gc<'_, T>,
    ) {
    }
}

unsafe impl<T: Trace + ?Sized> Trace for &T {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

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
    core::any::TypeId,
    rustc_hash::FxBuildHasher,
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

// Slices [T] cannot be allocated directly in the GC.
unsafe impl<T: Trace> Trace for [T] {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

// Box<T> where T: ?Sized. Box is always Sized even for DST contents.
unsafe impl<T: Trace + ?Sized> Trace for Box<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        (**self).trace(tracer);
    }
}

#[cfg(feature = "thin-vec")]
unsafe impl<T: Trace> Trace for thin_vec::ThinVec<T> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
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

// PhantomData<T> doesn't trace T, so T need not implement Trace.
unsafe impl<T: 'static> Trace for PhantomData<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

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

unsafe impl<T: ToOwned + Trace + ?Sized + 'static> Trace for Cow<'static, T>
where
    T::Owned: Trace,
{
    // T is already 'static so we can use it directly as the proxy.
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

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace, E: Trace> Trace for (A, B, C, D, E) {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
        self.4.trace(tracer);
    }
}

// Rc/Arc are reference-counted, not GC-traced. They cannot contain live Gc
// pointers (that would create a cycle the GC cannot see).
unsafe impl<T: ?Sized + 'static> Trace for rust_alloc::rc::Rc<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

unsafe impl<T: ?Sized + 'static> Trace for rust_alloc::sync::Arc<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// K is not traced (BTreeMap keys are immutable).
unsafe impl<K: 'static, V: Trace> Trace for BTreeMap<K, V> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.values() {
            v.trace(tracer);
        }
    }
}

// BTreeSet keys are never traced.
unsafe impl<T: 'static> Trace for BTreeSet<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// str is a DST, so we cannot allocate it directly. Use String as the Sized proxy.
unsafe impl Trace for str {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "icu")]
unsafe impl Trace for icu_locale_core::LanguageIdentifier {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "icu")]
unsafe impl Trace for icu_locale_core::Locale {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "either")]
unsafe impl<L: Trace, R: Trace> Trace for either::Either<L, R> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        match self {
            either::Either::Left(l) => l.trace(tracer),
            either::Either::Right(r) => r.trace(tracer),
        }
    }
}

#[cfg(feature = "arrayvec")]
unsafe impl<T: Trace, const N: usize> Trace for arrayvec::ArrayVec<T, N> {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self {
            v.trace(tracer);
        }
    }
}

unsafe impl<K: Trace, V: Trace, S: 'static> Trace for hashbrown::hash_map::HashMap<K, V, S> {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for (k, v) in self {
            k.trace(tracer);
            v.trace(tracer);
        }
    }
}
// Finalize is already implemented in common.rs

unsafe impl<T: Trace, S: 'static> Trace for hashbrown::hash_set::HashSet<T, S> {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self {
            v.trace(tracer);
        }
    }
}
// Finalize is already implemented in common.rs

unsafe impl<T: Trace> Trace for rust_alloc::collections::BinaryHeap<T> {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {
        // BinaryHeap has no iter_mut(); the null collector's trace is a no-op
        // so no values need to be visited here.
    }
}
// Finalize is already implemented in common.rs

#[cfg(feature = "std")]
unsafe impl Trace for std::path::Path {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl Trace for std::path::PathBuf {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl Trace for std::time::Instant {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl Trace for std::time::SystemTime {
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl<K: Trace, V: Trace, S: 'static> Trace for std::collections::HashMap<K, V, S> {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for (k, v) in self {
            k.trace(tracer);
            v.trace(tracer);
        }
    }
}

#[cfg(feature = "std")]
unsafe impl<T: Trace, S: 'static> Trace for std::collections::HashSet<T, S> {
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self {
            v.trace(tracer);
        }
    }
}

empty_trace!(
    core::sync::atomic::AtomicBool,
    core::sync::atomic::AtomicI8,
    core::sync::atomic::AtomicU8,
    core::sync::atomic::AtomicI16,
    core::sync::atomic::AtomicU16,
    core::sync::atomic::AtomicI32,
    core::sync::atomic::AtomicU32,
    core::sync::atomic::AtomicIsize,
    core::sync::atomic::AtomicUsize,
    core::sync::atomic::AtomicI64,
    core::sync::atomic::AtomicU64
);
