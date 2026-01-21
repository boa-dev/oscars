//! An Ephemeron implementation

use core::{any::TypeId, marker::PhantomData};

use crate::{
    alloc::arena2::ArenaHeapItem,
    collectors::mark_sweep::{
        CollectionState, ErasedEphemeron, MarkSweepGarbageCollector, TraceColor,
        internals::{GcBox, WeakGcBox, gc_header::HeaderColor},
        trace::Trace,
    },
};

use crate::collectors::mark_sweep::Finalize;

// TODO: key's GcBox should be notably a weak box
pub struct Ephemeron<K: Trace + ?Sized + 'static, V: Trace + 'static> {
    pub(crate) value: GcBox<V>,
    vtable: &'static EphemeronVTable,
    pub(crate) key: WeakGcBox<K>,
}

// NOTE: There is going to be an issue here in that we initialize the GC
// box to the wrong state.
//
// So we either need the color to be global that is provided to the allocation
// or we need state access
impl<K: Trace, V: Trace> Ephemeron<K, V> {
    pub fn new_in(key: K, value: V, collector: &mut MarkSweepGarbageCollector) -> Self
    where
        K: Sized,
    {
        let key = WeakGcBox::new_in(key, &collector.state);
        let value = GcBox::new_in(value, &collector.state);
        let vtable = vtable_of::<K, V>();
        Self { key, value, vtable }
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
    }

    impl<K: Trace + 'static, V: Trace + 'static> HasVTable for EphemeronMarker<K, V> {
        const VTABLE: &'static EphemeronVTable = &EphemeronVTable {
            trace_fn: EphemeronMarker::<K, V>::trace_fn::<K, V>,
            drop_fn: EphemeronMarker::<K, V>::drop_fn::<K, V>,
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

pub struct EphemeronVTable {
    trace_fn: EphemeronTraceFn,
    drop_fn: EphemeronDropFn,
    _key_type_id: TypeId,
    _key_size: usize,
    _value_type_id: TypeId,
    _value_size: usize,
}
