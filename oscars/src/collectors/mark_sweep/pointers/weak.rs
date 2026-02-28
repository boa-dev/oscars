use crate::{
    alloc::arena2::ArenaPointer,
    collectors::mark_sweep::{MarkSweepGarbageCollector, Trace, internals::Ephemeron},
};

#[repr(transparent)]
pub struct WeakGc<T: Trace + 'static> {
    inner_ptr: ArenaPointer<'static, Ephemeron<T, ()>>,
}

impl<T: Trace> WeakGc<T> {
    pub fn new_in(value: &super::Gc<T>, collector: &mut MarkSweepGarbageCollector) -> Self
    {
        let ephemeron = Ephemeron::new_in(value, (), collector);
        let inner_ptr = collector.alloc_epemeron_with_collection(ephemeron);
        Self { inner_ptr }
    }
}
