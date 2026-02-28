//! An Ephemeron implementation

use core::{any::TypeId, marker::PhantomData};

use crate::{
    alloc::arena2::ArenaHeapItem,
    collectors::mark_sweep::{
        CollectionState, ErasedEphemeron, MarkSweepGarbageCollector, TraceColor,
        internals::{GcBox, WeakGcBox, gc_header::HeaderColor},
        pointers::Gc,
        trace::Trace,
    },
};

use crate::collectors::mark_sweep::Finalize;

pub struct Ephemeron<K: Trace + ?Sized + 'static, V: Trace + 'static> {
    pub(crate) value: GcBox<V>,
    vtable: &'static EphemeronVTable,
    pub(crate) key: WeakGcBox<K>,
}

impl<K: Trace, V: Trace> Ephemeron<K, V> {
    // Creates a new [`Ephemeron`] with given key and value
    //
    // The [`WeakGcBox`] for the key is created internally from the provided [`Gc`] pointer
    pub fn new_in(key: &Gc<K>, value: V, collector: &mut MarkSweepGarbageCollector) -> Self {
        let weak_key = WeakGcBox::new(key.inner_ptr);
        let value = GcBox::new(value, &collector.state);
        let vtable = vtable_of::<K, V>();
        Self {
            key: weak_key,
            value,
            vtable,
        }
    }

    pub fn key(&self) -> &K {
        self.key.value()
    }

    pub fn value(&self) -> &V {
        self.value.value()
    }

    pub fn is_reachable(&self, color: TraceColor) -> bool {
        self.key.is_reachable(color)
    }

    pub(crate) fn set_unmarked(&self, state: &CollectionState) {
        self.key.set_unmarked(state);
    }
}

impl<K: Trace, V: Trace> Ephemeron<K, V> {
    pub(crate) fn trace_fn(&self) -> EphemeronTraceFn {
        self.vtable.trace_fn
    }

    pub(crate) fn drop_fn(&self) -> EphemeronDropFn {
        self.vtable.drop_fn
    }

    pub(crate) fn is_reachable_fn(&self) -> EphemeronIsReachableFn {
        self.vtable.is_reachable_fn
    }

    pub(crate) fn finalize_fn(&self) -> EphemeronFinalizeFn {
        self.vtable.finalize_fn
    }
}

impl<K: Trace, V: Trace> Finalize for Ephemeron<K, V> {}

unsafe impl<K: Trace, V: Trace> Trace for Ephemeron<K, V> {
    unsafe fn trace(&self, color: TraceColor) {
        // If object is not marked reachable, mark it as such.
        if !self.is_reachable(color) {
            self.key.mark(HeaderColor::Grey);
        }
    }

    fn run_finalizer(&self) {
        Finalize::finalize(self.key());
        Finalize::finalize(self.value());
    }
}

// Workaround: https://users.rust-lang.org/t/custom-vtables-with-integers/78508
pub(crate) const fn vtable_of<K: Trace + 'static, V: Trace + 'static>() -> &'static EphemeronVTable
{
    pub struct EphemeronMarker<K: Trace + 'static, V: Trace + 'static>((), PhantomData<(K, V)>);

    impl<K: Trace + 'static, V: Trace + 'static> Finalize for EphemeronMarker<K, V> {}

    unsafe impl<K: Trace + 'static, V: Trace + 'static> Trace for EphemeronMarker<K, V> {
        unsafe fn trace(&self, _: TraceColor) {}
        fn run_finalizer(&self) {}
    }

    trait HasVTable: Trace + Sized + 'static {
        const VTABLE: &'static EphemeronVTable;

        unsafe fn trace_fn<K: Trace + 'static, V: Trace + 'static>(
            this: ErasedEphemeron,
            color: TraceColor,
        ) {
            // SAFETY: The caller must ensure that the passed erased pointer is `GcBox<Self>`.
            let ephemeron = unsafe {
                this.cast::<ArenaHeapItem<Ephemeron<K, V>>>()
                    .as_ref()
                    .value()
            };

            // SAFETY: The implementor must ensure that `trace` is correctly implemented.
            unsafe {
                ephemeron.key.trace(color);
                ephemeron.value.trace(color);
            }
        }

        // SAFETY: The caller must ensure that the passed erased pointer is `GcBox<Self>`.
        unsafe fn drop_fn<K: Trace + 'static, V: Trace + 'static>(this: ErasedEphemeron) {
            // SAFETY: The caller must ensure that the passed erased pointer is `GcBox<Self>`.
            let mut this = this.cast::<ArenaHeapItem<Ephemeron<K, V>>>();

            // SAFETY: The caller must ensure the erased pointer is not dropped or deallocated.
            unsafe { this.as_mut().mark_dropped() };
        }

        // SAFETY: Cast back to concrete types to check reachability
        unsafe fn is_reachable_fn<K: Trace + 'static, V: Trace + 'static>(
            this: ErasedEphemeron,
            color: TraceColor,
        ) -> bool {
            // SAFETY: The caller must ensure that the passed erased pointer is
            // `ArenaHeapItem<Ephemeron<K, V>>`
            let ephemeron = unsafe {
                this.cast::<ArenaHeapItem<Ephemeron<K, V>>>()
                    .as_ref()
                    .value()
            };
            ephemeron.is_reachable(color)
        }

        // SAFETY: Cast back to concrete types to run finalizers
        unsafe fn finalize_fn<K: Trace + 'static, V: Trace + 'static>(this: ErasedEphemeron) {
            // SAFETY: The caller must ensure that the passed erased pointer is
            // `ArenaHeapItem<Ephemeron<K, V>>`
            let ephemeron = unsafe {
                this.cast::<ArenaHeapItem<Ephemeron<K, V>>>()
                    .as_ref()
                    .value()
            };
            Finalize::finalize(ephemeron.key());
            Finalize::finalize(ephemeron.value());
        }
    }

    impl<K: Trace + 'static, V: Trace + 'static> HasVTable for EphemeronMarker<K, V> {
        const VTABLE: &'static EphemeronVTable = &EphemeronVTable {
            trace_fn: EphemeronMarker::<K, V>::trace_fn::<K, V>,
            drop_fn: EphemeronMarker::<K, V>::drop_fn::<K, V>,
            is_reachable_fn: EphemeronMarker::<K, V>::is_reachable_fn::<K, V>,
            finalize_fn: EphemeronMarker::<K, V>::finalize_fn::<K, V>,
            _key_type_id: TypeId::of::<K>(),
            _key_size: size_of::<WeakGcBox<K>>(),
            _value_type_id: TypeId::of::<V>(),
            _value_size: size_of::<GcBox<V>>(),
        };
    }

    EphemeronMarker::<K, V>::VTABLE
}

type EphemeronTraceFn = unsafe fn(this: ErasedEphemeron, color: TraceColor);
type EphemeronDropFn = unsafe fn(this: ErasedEphemeron);
type EphemeronIsReachableFn = unsafe fn(this: ErasedEphemeron, color: TraceColor) -> bool;
type EphemeronFinalizeFn = unsafe fn(this: ErasedEphemeron);

pub struct EphemeronVTable {
    trace_fn: EphemeronTraceFn,
    drop_fn: EphemeronDropFn,
    is_reachable_fn: EphemeronIsReachableFn,
    finalize_fn: EphemeronFinalizeFn,
    _key_type_id: TypeId,
    _key_size: usize,
    _value_type_id: TypeId,
    _value_size: usize,
}
