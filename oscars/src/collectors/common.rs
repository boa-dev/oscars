//! Common types shared across all mark-and-sweep collector implementations.

use core::any::TypeId;
use core::cell::{Cell, OnceCell};
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

/// Substitute for the [`Drop`] trait for garbage collected types
///
/// Implement this to run cleanup logic before the GC frees an object.
/// The default implementation is a no-op
pub trait Finalize {
    /// Cleanup logic for a type
    fn finalize(&self) {}
}

// primitive and standard library blanket impls

macro_rules! simple_finalize {
    ($($T:ty),* $(,)?) => {
        $( impl Finalize for $T {} )*
    }
}

simple_finalize![
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
    NonZeroU128,
];

#[cfg(target_has_atomic = "8")]
simple_finalize![atomic::AtomicBool, atomic::AtomicI8, atomic::AtomicU8];

#[cfg(target_has_atomic = "16")]
simple_finalize![atomic::AtomicI16, atomic::AtomicU16];

#[cfg(target_has_atomic = "32")]
simple_finalize![atomic::AtomicI32, atomic::AtomicU32];

#[cfg(target_has_atomic = "64")]
simple_finalize![atomic::AtomicI64, atomic::AtomicU64];

#[cfg(target_has_atomic = "ptr")]
simple_finalize![atomic::AtomicIsize, atomic::AtomicUsize];

impl<T: ?Sized> Finalize for &'static T {}

impl<T: Finalize, const N: usize> Finalize for [T; N] {}

// Function pointer tuples, provide `Finalize` for function types.
macro_rules! fn_finalize_one {
    ($ty:ty $(,$args:ident)*) => {
        impl<Ret $(,$args)*> Finalize for $ty {}
    }
}
macro_rules! fn_finalize_group {
    () => {
        fn_finalize_one!(extern "Rust" fn () -> Ret);
        fn_finalize_one!(extern "C"    fn () -> Ret);
        fn_finalize_one!(unsafe extern "Rust" fn () -> Ret);
        fn_finalize_one!(unsafe extern "C"    fn () -> Ret);
    };
    ($($args:ident),*) => {
        fn_finalize_one!(extern "Rust" fn ($($args),*) -> Ret, $($args),*);
        fn_finalize_one!(extern "C"    fn ($($args),*) -> Ret, $($args),*);
        fn_finalize_one!(extern "C"    fn ($($args),*, ...) -> Ret, $($args),*);
        fn_finalize_one!(unsafe extern "Rust" fn ($($args),*) -> Ret, $($args),*);
        fn_finalize_one!(unsafe extern "C"    fn ($($args),*) -> Ret, $($args),*);
        fn_finalize_one!(unsafe extern "C"    fn ($($args),*, ...) -> Ret, $($args),*);
    }
}

macro_rules! tuple_finalize {
    () => {};
    ($($args:ident),*) => {
        impl<$($args),*> Finalize for ($($args,)*) {}
    }
}

macro_rules! type_arg_impls {
    ($(($($args:ident),*);)*) => {
        $(
            fn_finalize_group!($($args),*);
            tuple_finalize!($($args),*);
        )*
    }
}

type_arg_impls![
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

impl<T: Finalize + ?Sized> Finalize for Box<T> {}
impl<T: Finalize> Finalize for Box<[T]> {}
impl<T: Finalize> Finalize for Vec<T> {}

#[cfg(feature = "thin-vec")]
impl<T: Finalize> Finalize for thin_vec::ThinVec<T> {}

impl<T: Finalize> Finalize for Option<T> {}
impl<T: Finalize, E: Finalize> Finalize for Result<T, E> {}
impl<T: Ord + Finalize> Finalize for BinaryHeap<T> {}
impl<K: Finalize, V: Finalize> Finalize for BTreeMap<K, V> {}
impl<T: Finalize> Finalize for BTreeSet<T> {}
impl<T: Finalize> Finalize for LinkedList<T> {}
impl<T: Finalize> Finalize for VecDeque<T> {}

use core::hash::{BuildHasher, Hash};
impl<K: Eq + Hash + Finalize, V: Finalize, S: BuildHasher> Finalize
    for hashbrown::hash_map::HashMap<K, V, S>
{
}

#[cfg(feature = "std")]
impl<K: Eq + Hash + Finalize, V: Finalize, S: BuildHasher> Finalize for HashMap<K, V, S> {}

#[cfg(feature = "std")]
impl<T: Eq + Hash + Finalize, S: BuildHasher> Finalize for HashSet<T, S> {}

impl<T: Finalize> Finalize for Cell<Option<T>> {}
impl<T: Finalize> Finalize for OnceCell<T> {}
impl<T: ToOwned + Finalize + ?Sized> Finalize for Cow<'static, T> where T::Owned: Finalize {}

impl<T> Finalize for PhantomData<T> {}
