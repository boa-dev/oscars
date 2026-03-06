// `WeakGc<T>` uses `Ephemeron<T, ()>`, this allocates two GcBox headers
// per weak pointer. This overhead is acceptable for now but could be
// optimized in the future
use crate::{
    alloc::arena3::ArenaPointer,
    collectors::collector::Collector,
    collectors::mark_sweep::{Trace, internals::Ephemeron},
};
use core::marker::PhantomData;
use rust_alloc::rc::Rc;

/// A weak reference to a garbage-collected value.
///
/// # Thread Safety
///
/// `WeakGc<T>` is deliberately `!Send` and `!Sync`. The garbage collector
/// relies on non-atomic interior mutability (`Cell`) for header metadata.
/// Moving a `WeakGc<T>` across threads would create data races on
/// `GcHeader` fields, which is undefined behavior.
#[repr(transparent)]
pub struct WeakGc<T: Trace + 'static> {
    inner_ptr: ArenaPointer<'static, Ephemeron<T, ()>>,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl<T: Trace> WeakGc<T> {
    pub fn new_in<C: Collector>(value: &super::Gc<T>, collector: &C) -> Self
    where
        T: Sized,
    {
        let inner_ptr = collector
            .alloc_ephemeron_node(value, ())
            .expect("Failed to allocate Ephemeron node");

        // SAFETY: safe because the gc tracks this
        let inner_ptr = unsafe { inner_ptr.extend_lifetime() };

        Self {
            inner_ptr,
            _not_send_sync: PhantomData,
        }
    }
}
