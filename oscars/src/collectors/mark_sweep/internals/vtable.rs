use core::any::TypeId;

use crate::alloc::arena3::ArenaHeapItem;

use crate::collectors::mark_sweep::{GcBox, GcErasedPointer, Trace, TraceColor};

// Workaround: https://users.rust-lang.org/t/custom-vtables-with-integers/78508
pub(crate) const fn vtable_of<T: Trace + 'static>() -> &'static VTable {
    trait HasVTable: Trace + Sized + 'static {
        const VTABLE: &'static VTable;

        unsafe fn trace_fn(this: GcErasedPointer, color: TraceColor) {
            // SAFETY: The caller must ensure that the passed erased pointer is `GcBox<Self>`.
            let value = unsafe { this.cast::<ArenaHeapItem<GcBox<Self>>>().as_ref().value() };

            // SAFETY: The implementor must ensure that `trace` is correctly implemented.
            unsafe {
                Trace::trace(value, color);
            }
        }

        // SAFETY: The caller must ensure that the passed erased pointer is `GcBox<Self>`.
        unsafe fn drop_fn(this: GcErasedPointer) {
            // SAFETY: The caller must ensure that the passed erased pointer is `GcBox<Self>`.
            let mut this = this.cast::<ArenaHeapItem<GcBox<Self>>>();

            // SAFETY: The caller must ensure the erased pointer is not dropped or deallocated.
            unsafe { core::ptr::drop_in_place(this.as_mut()) };
        }
    }

    impl<T: Trace + 'static> HasVTable for T {
        const VTABLE: &'static VTable = &VTable {
            trace_fn: T::trace_fn,
            drop_fn: T::drop_fn,
            type_id: TypeId::of::<T>(),
            size: size_of::<GcBox<T>>(),
        };
    }

    T::VTABLE
}

pub(crate) type TraceFn = unsafe fn(this: GcErasedPointer, color: TraceColor);
pub(crate) type DropFn = unsafe fn(this: GcErasedPointer);

#[derive(Debug)]
pub(crate) struct VTable {
    trace_fn: TraceFn,
    drop_fn: DropFn,
    type_id: TypeId,
    size: usize,
}

impl VTable {
    pub(crate) fn trace_fn(&self) -> TraceFn {
        self.trace_fn
    }

    pub(crate) fn drop_fn(&self) -> DropFn {
        self.drop_fn
    }

    pub(crate) const fn type_id(&self) -> TypeId {
        self.type_id
    }

    pub(crate) fn size(&self) -> usize {
        self.size
    }
}
