// `WeakGc<T>` uses `Ephemeron<T, ()>`, this allocates two GcBox headers
// per weak pointer. This overhead is acceptable for now but could be 
// optimized in the future
use crate::{
    alloc::arena2::ArenaPointer,
    collectors::collector::Collector,
    collectors::mark_sweep::{Trace, internals::Ephemeron},
};

#[repr(transparent)]
pub struct WeakGc<T: Trace + 'static> {
    inner_ptr: ArenaPointer<'static, Ephemeron<T, ()>>,
}

impl<T: Trace> WeakGc<T> {
    pub fn new_in<C: Collector>(value: T, collector: &C) -> Self
    where
        T: Sized,
    {
        let inner_ptr = collector
            .alloc_ephemeron_node(value, ())
            .expect("Failed to allocate Ephemeron node");
        Self { inner_ptr }
    }
}
