//! An Ephemeron implementation

use core::marker::PhantomData;

use crate::{
    alloc::arena3::ArenaHeapItem,
    collectors::mark_sweep::{
        ErasedEphemeron, TraceColor,
        internals::{GcBox, WeakGcBox},
        pointers::Gc,
        trace::Trace,
    },
};

use crate::collectors::mark_sweep::Finalize;

pub struct Ephemeron<K: Trace + ?Sized + 'static, V: Trace + 'static> {
    pub(crate) value: GcBox<V>,
    vtable: &'static EphemeronVTable,
    pub(crate) key: WeakGcBox<K>,
    pub(crate) active: core::cell::Cell<bool>,
}

impl<K: Trace, V: Trace> Ephemeron<K, V> {
    // create an Ephemeron with the given GC trace color
    pub(crate) fn new(key: &Gc<K>, value: V, color: TraceColor) -> Self {
        let weak_key = WeakGcBox::new(key.inner_ptr);
        let value = GcBox::new_in(value, color);
        let vtable = vtable_of::<K, V>();
        Self {
            key: weak_key,
            value,
            vtable,
            active: core::cell::Cell::new(true),
        }
    }

    pub fn key(&self) -> &K {
        self.key.value()
    }

    pub fn value(&self) -> &V {
        self.value.value()
    }

    pub fn is_reachable(&self, color: TraceColor) -> bool {
        self.active.get() && self.key.is_reachable(color)
    }

    pub(crate) fn invalidate(&self) {
        self.active.set(false);
    }
}

impl<K: Trace, V: Trace> Ephemeron<K, V> {
    pub(crate) fn trace_fn(&self) -> EphemeronTraceFn {
        self.vtable.trace_fn
    }

    pub(crate) fn is_reachable_fn(
        &self,
    ) -> unsafe fn(this: ErasedEphemeron, color: TraceColor) -> bool {
        self.vtable.is_reachable_fn
    }

    pub(crate) fn finalize_fn(&self) -> unsafe fn(this: ErasedEphemeron) {
        self.vtable.finalize_fn
    }

    pub(crate) fn drop_fn(&self) -> EphemeronDropFn {
        self.vtable.drop_fn
    }
}

impl<K: Trace, V: Trace> Finalize for Ephemeron<K, V> {}

// NOTE on Trace for Ephemeron:
// this impl just satisfies `Trace` bounds for the allocator framework
// actual GC tracing routes through the `EphemeronVTable`.
// do not add logic here as it panics if called directly
unsafe impl<K: Trace, V: Trace> Trace for Ephemeron<K, V> {
    unsafe fn trace(&self, _color: TraceColor) {
        panic!("Trace::trace called on Ephemeron directly; must be dispatched via vtable");
    }

    fn run_finalizer(&self) {
        Finalize::finalize(&self.key);
        Finalize::finalize(&self.value);
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
            // SAFETY: The caller must ensure that the passed erased pointer is `ArenaHeapItem<Ephemeron<K, V>>`.
            let mut this = this.cast::<ArenaHeapItem<Ephemeron<K, V>>>();

            // drop the Ephemeron value in place, the arena bitmap is cleared
            // by the sweep loop after this function returns
            unsafe { core::ptr::drop_in_place(this.as_mut()) };
        }
    }

    impl<K: Trace + 'static, V: Trace + 'static> HasVTable for EphemeronMarker<K, V> {
        const VTABLE: &'static EphemeronVTable = &EphemeronVTable {
            trace_fn: EphemeronMarker::<K, V>::trace_fn::<K, V>,
            drop_fn: EphemeronMarker::<K, V>::drop_fn::<K, V>,
            is_reachable_fn: |this, color| unsafe {
                let ephemeron = this
                    .cast::<ArenaHeapItem<Ephemeron<K, V>>>()
                    .as_ref()
                    .value();
                ephemeron.active.get() && ephemeron.key.is_reachable(color)
            },
            finalize_fn: |this| unsafe {
                let ephemeron = this
                    .cast::<ArenaHeapItem<Ephemeron<K, V>>>()
                    .as_ref()
                    .value();
                Finalize::finalize(ephemeron);
            },
        };
    }

    EphemeronMarker::<K, V>::VTABLE
}

type EphemeronTraceFn = unsafe fn(this: ErasedEphemeron, color: TraceColor);
type EphemeronDropFn = unsafe fn(this: ErasedEphemeron);

pub struct EphemeronVTable {
    trace_fn: EphemeronTraceFn,
    drop_fn: EphemeronDropFn,
    is_reachable_fn: unsafe fn(this: ErasedEphemeron, color: TraceColor) -> bool,
    finalize_fn: unsafe fn(this: ErasedEphemeron),
}
