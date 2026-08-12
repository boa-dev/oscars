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
    type StaticId: 'static + Trace<StaticId = Self::StaticId>;
    /// Marks all `Gc` pointers reachable from `self`.
    ///
    /// # Safety
    ///
    /// Must only be called by the garbage collector. Implementors must call
    /// `Tracer::mark` on all reachable `Gc` fields and avoid other unsafe operations
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
    pub fn mark<T: Trace + ?Sized>(&mut self, gc: &Gc<'_, T>) {
        // SAFETY: `gc.ptr` is a valid `PoolItem<GcBox<T>>`.
        unsafe {
            let gc_box = &(*gc.ptr.as_ptr().as_ptr()).0;
            if gc_box.color.get() == GcColor::White {
                gc_box.color.set(GcColor::Gray);
                self.worklist
                    .push((gc.ptr.as_ptr().cast::<u8>(), gc_box.trace_fn));
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

// For &T, the StaticId is &'static T::StaticId. Since &U is always Sized, this
// satisfies the Sized requirement on StaticId even when T::StaticId is a DST.
// We add the bound T::StaticId: Sized to keep things simple and unambiguous.
unsafe impl<T: Trace + ?Sized> Trace for &T
where
    T::StaticId: Sized,
{
    type StaticId = &'static T::StaticId;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// primitive + std-lib Trace impls

macro_rules! empty_trace {
    ($($T:ty),* $(,)?) => {
        $(
            unsafe impl Trace for $T {
                type StaticId = $T;
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
    core::sync::atomic::AtomicIsize,
    core::sync::atomic::AtomicUsize,
];

// str is a DST; we cannot allocate it directly. Use String as the Sized proxy.
unsafe impl Trace for str {
    type StaticId = String;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

unsafe impl<T: Trace, const N: usize> Trace for [T; N] {
    type StaticId = [T::StaticId; N];
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

// Slices [T] cannot be allocated directly in the GC. Their StaticId is a
// Vec, which is always Sized and avoids Box fixed point divergence.
unsafe impl<T: Trace> Trace for [T] {
    type StaticId = Vec<T::StaticId>;
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self {
            v.trace(tracer);
        }
    }
}

// Box<T> where T: ?Sized. Box is always Sized even for DST contents.
// We require T::StaticId: Sized to produce a concrete Sized StaticId.
unsafe impl<T: Trace + ?Sized> Trace for Box<T>
where
    T::StaticId: Sized,
{
    type StaticId = Box<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        (**self).trace(tracer);
    }
}

unsafe impl<T: Trace> Trace for Option<T> {
    type StaticId = Option<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Some(v) = self {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace, E: Trace> Trace for Result<T, E> {
    type StaticId = Result<T::StaticId, E::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        match self {
            Ok(v) => v.trace(tracer),
            Err(e) => e.trace(tracer),
        }
    }
}

unsafe impl<T: Trace> Trace for Vec<T> {
    type StaticId = Vec<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

#[cfg(feature = "thin-vec")]
unsafe impl<T: Trace> Trace for thin_vec::ThinVec<T> {
    type StaticId = thin_vec::ThinVec<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace> Trace for VecDeque<T> {
    type StaticId = VecDeque<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace> Trace for LinkedList<T> {
    type StaticId = LinkedList<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}

// PhantomData<T> doesn't trace T, so T need not implement Trace.
// For StaticId we require T: 'static so the proxy type itself is 'static.
unsafe impl<T: 'static> Trace for PhantomData<T> {
    type StaticId = PhantomData<T>;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

unsafe impl Trace for core::any::TypeId {
    type StaticId = core::any::TypeId;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// Cell<Option<T>> requires T: Copy to safely read the value via Cell::get().
// For non-Copy types, use GcRefCell instead.
unsafe impl<T: Trace + Default> Trace for Cell<T>
where
    T::StaticId: Default,
{
    type StaticId = Cell<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        let v = self.take();
        v.trace(tracer);
        self.set(v);
    }
}

unsafe impl<T: Trace> Trace for OnceCell<T> {
    type StaticId = OnceCell<T::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Some(v) = self.get() {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: ToOwned + Trace + ?Sized + 'static> Trace for Cow<'static, T>
where
    T::Owned: Trace,
    T::StaticId: ToOwned,
{
    // T is already 'static so we can use it directly as the proxy.
    type StaticId = Cow<'static, T>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        if let Cow::Owned(v) = self {
            v.trace(tracer);
        }
    }
}

unsafe impl<A: Trace> Trace for (A,) {
    type StaticId = (A::StaticId,);
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace> Trace for (A, B) {
    type StaticId = (A::StaticId, B::StaticId);
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace> Trace for (A, B, C) {
    type StaticId = (A::StaticId, B::StaticId, C::StaticId);
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace> Trace for (A, B, C, D) {
    type StaticId = (A::StaticId, B::StaticId, C::StaticId, D::StaticId);
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace, E: Trace> Trace for (A, B, C, D, E) {
    type StaticId = (
        A::StaticId,
        B::StaticId,
        C::StaticId,
        D::StaticId,
        E::StaticId,
    );
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
        self.4.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace, E: Trace, F: Trace> Trace
    for (A, B, C, D, E, F)
{
    type StaticId = (
        A::StaticId,
        B::StaticId,
        C::StaticId,
        D::StaticId,
        E::StaticId,
        F::StaticId,
    );
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
        self.4.trace(tracer);
        self.5.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace, E: Trace, F: Trace, G: Trace> Trace
    for (A, B, C, D, E, F, G)
{
    type StaticId = (
        A::StaticId,
        B::StaticId,
        C::StaticId,
        D::StaticId,
        E::StaticId,
        F::StaticId,
        G::StaticId,
    );
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
        self.4.trace(tracer);
        self.5.trace(tracer);
        self.6.trace(tracer);
    }
}

unsafe impl<A: Trace, B: Trace, C: Trace, D: Trace, E: Trace, F: Trace, G: Trace, H: Trace> Trace
    for (A, B, C, D, E, F, G, H)
{
    type StaticId = (
        A::StaticId,
        B::StaticId,
        C::StaticId,
        D::StaticId,
        E::StaticId,
        F::StaticId,
        G::StaticId,
        H::StaticId,
    );
    unsafe fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
        self.1.trace(tracer);
        self.2.trace(tracer);
        self.3.trace(tracer);
        self.4.trace(tracer);
        self.5.trace(tracer);
        self.6.trace(tracer);
        self.7.trace(tracer);
    }
}

// Rc and Arc do not contain Gc pointers (they use reference counting, not GC).
// If you need to store Gc pointers inside Rc/Arc, wrap them in a GC-allocated
// struct instead.
// Rc/Arc are reference-counted, not GC-traced. They cannot contain live Gc
// pointers (that would create a cycle the GC cannot see). StaticId uses the
// 'static-bounded form so TypeId is well-formed.
unsafe impl<T: ?Sized + 'static> Trace for rust_alloc::rc::Rc<T> {
    type StaticId = rust_alloc::rc::Rc<T>;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

unsafe impl<T: ?Sized + 'static> Trace for rust_alloc::sync::Arc<T> {
    type StaticId = rust_alloc::sync::Arc<T>;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

// K is not traced (BTreeMap keys are immutable); require K: 'static for StaticId.
unsafe impl<K: 'static, V: Trace> Trace for BTreeMap<K, V> {
    type StaticId = BTreeMap<K, V::StaticId>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.values() {
            v.trace(tracer);
        }
    }
}

// BTreeSet keys are never traced; require T: 'static for StaticId.
unsafe impl<T: 'static> Trace for BTreeSet<T> {
    type StaticId = BTreeSet<T>;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {
        // BTreeSet keys are immutable and cannot contain Gc pointers
        // that need tracing (Gc requires &mut self to trace).
    }
}

#[cfg(feature = "icu")]
mod icu_trace {
    use crate::collectors::mark_sweep_branded::{Trace, Tracer};
    use icu_locale_core::{LanguageIdentifier, Locale};

    unsafe impl Trace for LanguageIdentifier {
        type StaticId = LanguageIdentifier;
        #[inline]
        unsafe fn trace(&self, _tracer: &mut Tracer) {}
    }

    unsafe impl Trace for Locale {
        type StaticId = Locale;
        #[inline]
        unsafe fn trace(&self, _tracer: &mut Tracer) {}
    }
}

#[cfg(feature = "either")]
mod either_trace {
    use crate::collectors::mark_sweep_branded::{Trace, Tracer};

    unsafe impl<L: Trace, R: Trace> Trace for either::Either<L, R> {
        type StaticId = either::Either<L::StaticId, R::StaticId>;
        unsafe fn trace(&self, tracer: &mut Tracer) {
            match self {
                either::Either::Left(l) => l.trace(tracer),
                either::Either::Right(r) => r.trace(tracer),
            }
        }
    }
}

#[cfg(feature = "arrayvec")]
unsafe impl<T: Trace, const N: usize> Trace for arrayvec::ArrayVec<T, N> {
    type StaticId = arrayvec::ArrayVec<T::StaticId, N>;
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self {
            v.trace(tracer);
        }
    }
}

#[cfg(feature = "std")]
unsafe impl Trace for std::path::Path {
    type StaticId = std::path::PathBuf;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl Trace for std::path::PathBuf {
    type StaticId = std::path::PathBuf;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl Trace for std::time::Instant {
    type StaticId = std::time::Instant;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl Trace for std::time::SystemTime {
    type StaticId = std::time::SystemTime;
    #[inline]
    unsafe fn trace(&self, _tracer: &mut Tracer) {}
}

#[cfg(feature = "std")]
unsafe impl<K: Trace, V: Trace, S: 'static> Trace for std::collections::HashMap<K, V, S> {
    type StaticId = std::collections::HashMap<K::StaticId, V::StaticId, S>;
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
    type StaticId = std::collections::HashSet<T::StaticId, S>;
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self {
            v.trace(tracer);
        }
    }
}

unsafe impl<T: Trace> Trace for rust_alloc::collections::BinaryHeap<T> {
    type StaticId = rust_alloc::collections::BinaryHeap<T::StaticId>;
    #[inline]
    unsafe fn trace(&self, tracer: &mut Tracer) {
        for v in self.iter() {
            v.trace(tracer);
        }
    }
}
