// `WeakGc<T>` uses `Ephemeron<T, ()>`, this allocates two GcBox headers
// per weak pointer. This overhead is acceptable for now but could be
// optimized in the future
use crate::{
    alloc::arena2::ArenaPointer,
    collectors::mark_sweep_arena2::{Trace, internals::Ephemeron},
};

#[repr(transparent)]
pub struct WeakGc<T: Trace + 'static> {
    inner_ptr: ArenaPointer<'static, Ephemeron<T, ()>>,
}

impl<T: Trace> WeakGc<T> {
    pub fn new_in(
        value: &super::Gc<T>,
        collector: &crate::collectors::mark_sweep_arena2::MarkSweepGarbageCollector,
    ) -> Self
    where
        T: Sized,
    {
        let inner_ptr = collector
            .alloc_ephemeron_node(value, ())
            .expect("Failed to allocate Ephemeron node");

        // SAFETY: safe because the gc tracks this
        let inner_ptr: ArenaPointer<'static, Ephemeron<T, ()>> =
            unsafe { inner_ptr.extend_lifetime() };

        Self { inner_ptr }
    }
}
