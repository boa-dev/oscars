//! In the null collector, roots are a zero cost abstraction because nothing is ever collected
//! before the entire context is dropped.

use crate::{
    alloc::mempool3::PoolPointer,
    collectors::null_collector_branded::{
        gc::Gc, gc_box::GcBox, mutation_ctx::MutationContext, trace::Trace,
    },
};
use core::marker::PhantomData;

#[must_use = "dropping a root unregisters it from the GC"]
pub struct Root<'id, T: Trace> {
    gc_ptr: PoolPointer<'static, GcBox<T>>,
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace> Root<'id, T> {
    /// Creates a new root from a Gc pointer
    pub fn new(_mc: &MutationContext<'id, '_>, value: Gc<'_, T>) -> Self {
        Self {
            gc_ptr: value.ptr,
            _marker: PhantomData,
        }
    }

    /// Converts this root into a `Gc` pointer
    pub fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Gc<'gc, T> {
        Gc {
            ptr: self.gc_ptr,
            _marker: PhantomData,
        }
    }
}
