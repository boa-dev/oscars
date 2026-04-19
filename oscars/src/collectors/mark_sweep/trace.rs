use core::any::TypeId;
use core::cell::{Cell, OnceCell};
use core::hash::{BuildHasher, Hash};
use core::marker::PhantomData;
use core::num::{
    NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128, NonZeroIsize, NonZeroU8,
    NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128, NonZeroUsize,
};
use core::sync::atomic;

use rust_alloc::borrow::{Cow, ToOwned};
use rust_alloc::boxed::Box;
use rust_alloc::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};
use rust_alloc::rc::Rc;
use rust_alloc::string::String;
use rust_alloc::vec::Vec;

#[cfg(feature = "std")]
use std::collections::{HashMap, HashSet};

// Re-export the shared `Finalize` trait and all its stdlib blanket impls.
pub use crate::collectors::common::Finalize;

#[derive(Debug, Clone, Copy, Default)]
#[repr(u8)]
pub enum TraceColor {
    #[default]
    Black,
    White,
}

impl TraceColor {
    pub fn flip(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
}

/// The [`Trace`] trait for tracing Garbage collected values on the heap
///
/// # Safety
///
/// An incorrect tracing implementation may cause undefined behavior.
pub unsafe trait Trace: Finalize {
    /// The primary trace function of the trace trait
    ///
    /// # Safety
    ///
    /// A correct implementation of the tri-color tracing abstraction must account for
    /// self referential cycles.
    ///
    /// - An incorrect implementation may cause undefined behavior
    unsafe fn trace(&self, color: TraceColor);

    fn run_finalizer(&self);
}

/// Utility macro to define an empty implementation of [`Trace`].
///
/// Use this for marking types as not containing any `Trace` types.
#[macro_export]
macro_rules! empty_trace {
    () => {
        #[inline]
        unsafe fn trace(&self, _color: $crate::collectors::mark_sweep::TraceColor) {}
        #[inline]
        fn run_finalizer(&self) {
            $crate::collectors::mark_sweep::Finalize::finalize(self);
        }
    };
}

/// Utility macro to manually implement [`Trace`] on a type.
///
/// You define a `this` parameter name and pass in a body, which should call `mark` on every
/// traceable element inside the body. The mark implementation will automatically delegate to the
/// correct method on the argument.
///
/// # Safety
///
/// Misusing the `mark` function may result in Undefined Behaviour.
#[macro_export]
macro_rules! custom_trace {
    ($this:ident, $marker:ident, $body:expr) => {
        #[inline]
        unsafe fn trace(&self, color: $crate::collectors::mark_sweep::TraceColor) {
            let $marker = |it: &dyn $crate::collectors::mark_sweep::Trace| {
                // SAFETY: The implementor must ensure that `trace` is correctly implemented.
                unsafe {
                    $crate::collectors::mark_sweep::Trace::trace(it, color);
                }
            };
            let $this = self;
            $body
        }
        #[inline]
        fn run_finalizer(&self) {
            fn $marker<T: $crate::collectors::mark_sweep::Trace + ?Sized>(it: &T) {
                $crate::collectors::mark_sweep::Trace::run_finalizer(it);
            }
            $crate::collectors::mark_sweep::Finalize::finalize(self);
            let $this = self;
            $body
        }
    };
}

// SAFETY: 'static references don't need to be traced, since they live indefinitely.
unsafe impl<T: ?Sized> Trace for &'static T {
    empty_trace!();
}

macro_rules! simple_empty_trace {
    ($($T:ty),* $(,)?) => {
        $(
            // SAFETY:
            // Primitive types and string types don't have inner nodes that need to be marked.
            unsafe impl Trace for $T { empty_trace!(); }
        )*
    }
}

simple_empty_trace![
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
    TypeId,
    String,
    str,
    Rc<str>,
    NonZeroIsize,
    NonZeroUsize,
    NonZeroI8,
    NonZeroU8,
    NonZeroI16,
    NonZeroU16,
    NonZeroI32,
    NonZeroU32,
    NonZeroI64,
    NonZeroU64,
    NonZeroI128,
    NonZeroU128
];

#[cfg(target_has_atomic = "8")]
simple_empty_trace![atomic::AtomicBool, atomic::AtomicI8, atomic::AtomicU8];

#[cfg(target_has_atomic = "16")]
simple_empty_trace![atomic::AtomicI16, atomic::AtomicU16];

#[cfg(target_has_atomic = "32")]	
simple_empty_trace![atomic::AtomicI32, atomic::AtomicU32];

#[cfg(target_has_atomic = "64")]
simple_empty_trace![atomic::AtomicI64, atomic::AtomicU64];

#[cfg(target_has_atomic = "ptr")]
simple_empty_trace![atomic::AtomicIsize, atomic::AtomicUsize];

// SAFETY:
// All elements inside the array are correctly marked.
unsafe impl<T: Trace, const N: usize> Trace for [T; N] {
    custom_trace!(this, mark, {
        for v in this {
            mark(v);
        }
    });
}

macro_rules! fn_trace_one {
    ($ty:ty $(,$args:ident)*) => {
        // SAFETY:
        // Function pointers don't have inner nodes that need to be marked.
        unsafe impl<Ret $(,$args)*> Trace for $ty { empty_trace!(); }
    }
}
macro_rules! fn_trace_group {
    () => {
        fn_trace_one!(extern "Rust" fn () -> Ret);
        fn_trace_one!(extern "C" fn () -> Ret);
        fn_trace_one!(unsafe extern "Rust" fn () -> Ret);
        fn_trace_one!(unsafe extern "C" fn () -> Ret);
    };
    ($($args:ident),*) => {
        fn_trace_one!(extern "Rust" fn ($($args),*) -> Ret, $($args),*);
        fn_trace_one!(extern "C" fn ($($args),*) -> Ret, $($args),*);
        fn_trace_one!(extern "C" fn ($($args),*, ...) -> Ret, $($args),*);
        fn_trace_one!(unsafe extern "Rust" fn ($($args),*) -> Ret, $($args),*);
        fn_trace_one!(unsafe extern "C" fn ($($args),*) -> Ret, $($args),*);
        fn_trace_one!(unsafe extern "C" fn ($($args),*, ...) -> Ret, $($args),*);
    }
}

macro_rules! tuple_trace {
    () => {}; // () handled by simple_empty_trace!
    ($($args:ident),*) => {
        // SAFETY:
        // All elements inside the tuple are correctly marked.
        unsafe impl<$($args: $crate::collectors::mark_sweep::Trace),*> Trace for ($($args,)*) {
            custom_trace!(this, mark, {
                #[allow(non_snake_case, unused_unsafe, unused_mut)]
                let mut avoid_lints = |&($(ref $args,)*): &($($args,)*)| {
                    // SAFETY: The implementor must ensure a correct implementation.
                    unsafe { $(mark($args);)* }
                };
                avoid_lints(this)
            });
        }
    }
}

macro_rules! type_arg_trace_impls {
    ($(($($args:ident),*);)*) => {
        $(
            fn_trace_group!($($args),*);
            tuple_trace!($($args),*);
        )*
    }
}

type_arg_trace_impls![
    ();
    (A);
    (A, B);
    (A, B, C);
    (A, B, C, D);
    (A, B, C, D, E);
    (A, B, C, D, E, F);
    (A, B, C, D, E, F, G);
    (A, B, C, D, E, F, G, H);
    (A, B, C, D, E, F, G, H, I);
    (A, B, C, D, E, F, G, H, I, J);
    (A, B, C, D, E, F, G, H, I, J, K);
    (A, B, C, D, E, F, G, H, I, J, K, L);
];

// SAFETY: The inner value of the `Box` is correctly marked.
unsafe impl<T: Trace + ?Sized> Trace for Box<T> {
    #[inline]
    unsafe fn trace(&self, color: TraceColor) {
        // SAFETY: The implementor must ensure that `trace` is correctly implemented.
        unsafe {
            Trace::trace(&**self, color);
        }
    }

    #[inline]
    fn run_finalizer(&self) {
        Finalize::finalize(self);
        Trace::run_finalizer(&**self);
    }
}

// SAFETY: All the inner elements of the `Box` array are correctly marked.
unsafe impl<T: Trace> Trace for Box<[T]> {
    custom_trace!(this, mark, {
        for e in &**this {
            mark(e);
        }
    });
}

// SAFETY: All the inner elements of the `Vec` are correctly marked.
unsafe impl<T: Trace> Trace for Vec<T> {
    custom_trace!(this, mark, {
        for e in this {
            mark(e);
        }
    });
}

#[cfg(feature = "thin-vec")]
// SAFETY: All the inner elements of the `ThinVec` are correctly marked.
unsafe impl<T: Trace> Trace for thin_vec::ThinVec<T> {
    custom_trace!(this, mark, {
        for e in this {
            mark(e);
        }
    });
}

// SAFETY: The inner value of the `Option` is correctly marked.
unsafe impl<T: Trace> Trace for Option<T> {
    custom_trace!(this, mark, {
        if let Some(ref v) = *this {
            mark(v);
        }
    });
}

// SAFETY: Both inner values of the `Result` are correctly marked.
unsafe impl<T: Trace, E: Trace> Trace for Result<T, E> {
    custom_trace!(this, mark, {
        match *this {
            Ok(ref v) => mark(v),
            Err(ref v) => mark(v),
        }
    });
}

// SAFETY: All the elements of the `BinaryHeap` are correctly marked.
unsafe impl<T: Ord + Trace> Trace for BinaryHeap<T> {
    custom_trace!(this, mark, {
        for v in this {
            mark(v);
        }
    });
}

// SAFETY: All the elements of the `BTreeMap` are correctly marked.
unsafe impl<K: Trace, V: Trace> Trace for BTreeMap<K, V> {
    custom_trace!(this, mark, {
        for (k, v) in this {
            mark(k);
            mark(v);
        }
    });
}

// SAFETY: All the elements of the `BTreeSet` are correctly marked.
unsafe impl<T: Trace> Trace for BTreeSet<T> {
    custom_trace!(this, mark, {
        for v in this {
            mark(v);
        }
    });
}

// SAFETY: All the elements of the `HashMap` are correctly marked.
unsafe impl<K: Eq + Hash + Trace, V: Trace, S: BuildHasher> Trace
    for hashbrown::hash_map::HashMap<K, V, S>
{
    custom_trace!(this, mark, {
        for (k, v) in this {
            mark(k);
            mark(v);
        }
    });
}

#[cfg(feature = "std")]
// SAFETY: All the elements of the `HashMap` are correctly marked.
unsafe impl<K: Eq + Hash + Trace, V: Trace, S: BuildHasher> Trace for HashMap<K, V, S> {
    custom_trace!(this, mark, {
        for (k, v) in this {
            mark(k);
            mark(v);
        }
    });
}

#[cfg(feature = "std")]
// SAFETY: All the elements of the `HashSet` are correctly marked.
unsafe impl<T: Eq + Hash + Trace, S: BuildHasher> Trace for HashSet<T, S> {
    custom_trace!(this, mark, {
        for v in this {
            mark(v);
        }
    });
}

// SAFETY: All the elements of the `LinkedList` are correctly marked.
unsafe impl<T: Eq + Hash + Trace> Trace for LinkedList<T> {
    custom_trace!(this, mark, {
        #[allow(clippy::explicit_iter_loop)]
        for v in this.iter() {
            mark(v);
        }
    });
}

// SAFETY: A `PhantomData` doesn't have inner data that needs to be marked.
unsafe impl<T> Trace for PhantomData<T> {
    empty_trace!();
}

// SAFETY: All the elements of the `VecDeque` are correctly marked.
unsafe impl<T: Trace> Trace for VecDeque<T> {
    custom_trace!(this, mark, {
        for v in this {
            mark(v);
        }
    });
}

// SAFETY: 'static references don't need to be traced, since they live indefinitely, and the owned
// variant is correctly marked.
unsafe impl<T: ToOwned + Trace + ?Sized> Trace for Cow<'static, T>
where
    T::Owned: Trace,
{
    custom_trace!(this, mark, {
        if let Cow::Owned(v) = this {
            mark(v);
        }
    });
}

// SAFETY: Taking and setting is done in a single action, and recursive traces should find a `None`
// value instead of the original `T`, making this safe.
unsafe impl<T: Trace> Trace for Cell<Option<T>> {
    custom_trace!(this, mark, {
        if let Some(v) = this.take() {
            mark(&v);
            this.set(Some(v));
        }
    });
}

// SAFETY: We only trace the inner cell if the cell has a value.
unsafe impl<T: Trace> Trace for OnceCell<T> {
    custom_trace!(this, mark, {
        if let Some(v) = this.get() {
            mark(v);
        }
    });
}

/*
#[cfg(feature = "icu")]
mod icu {
    use icu_locale_core::{LanguageIdentifier, Locale};

    use crate::mark_sweep::{Finalize, Trace};

    impl Finalize for LanguageIdentifier {}

    // SAFETY: `LanguageIdentifier` doesn't have any traceable data.
    unsafe impl Trace for LanguageIdentifier {
        empty_trace!();
    }

    impl Finalize for Locale {}

    // SAFETY: `LanguageIdentifier` doesn't have any traceable data.
    unsafe impl Trace for Locale {
        empty_trace!();
    }
}

#[cfg(feature = "boa_string")]
mod boa_string_trace {
    use crate::mark_sweep::{Finalize, Trace};

    // SAFETY: `boa_string::JsString` doesn't have any traceable data.
    unsafe impl Trace for boa_string::JsString {
        empty_trace!();
    }

    impl Finalize for boa_string::JsString {}
}

#[cfg(feature = "either")]
mod either_trace {
    use crate::mark_sweep::{Finalize, Trace};

    impl<L: Trace, R: Trace> Finalize for either::Either<L, R> {}

    unsafe impl<L: Trace, R: Trace> Trace for either::Either<L, R> {
        custom_trace!(this, mark, {
            match this {
                either::Either::Left(l) => mark(l),
                either::Either::Right(r) => mark(r),
            }
        });
    }
}
*/
