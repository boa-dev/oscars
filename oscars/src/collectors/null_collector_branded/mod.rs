//! Lifetime branded null GC
#![cfg_attr(not(any(test, feature = "std")), allow(unused_imports))]

pub mod cell;
pub mod ephemeron;
pub mod gc;
pub mod gc_box;
pub mod mutation_ctx;
pub mod root;
pub mod trace;
pub mod weak;

pub use cell::GcRefCell;
pub use ephemeron::Ephemeron;
pub use gc::Gc;
pub use mutation_ctx::MutationContext;
pub use root::Root;
pub use trace::{Finalize, Trace, Tracer};
pub use weak::WeakGc;

use crate::alloc::mempool3::{PoolAllocError, PoolAllocator, PoolPointer};
use core::cell::RefCell;
use core::marker::PhantomData;
use core::ptr::NonNull;
use gc_box::{DropFn, GcBox};
use rust_alloc::vec::Vec;

pub(crate) struct Collector {
    // SAFETY: We use 'static here because the PoolAllocator owns its memory,
    // and we ensure that `Gc` objects and pool allocations do not outlive
    // the `Collector` instance
    pub(crate) pool: RefCell<PoolAllocator<'static>>,
}

impl Collector {
    fn new() -> Self {
        Self {
            pool: RefCell::new(PoolAllocator::default()),
        }
    }

    /// Allocates a value from the pool.
    pub(crate) fn try_alloc<'gc, T: trace::Trace + trace::Finalize + 'gc>(
        &'gc self,
        value: T,
    ) -> Result<Gc<'gc, T>, PoolAllocError> {
        unsafe fn drop_and_free<T: trace::Trace + trace::Finalize>(
            pool: &mut PoolAllocator<'static>,
            ptr: NonNull<u8>,
        ) {
            use crate::alloc::mempool3::PoolItem;
            unsafe {
                let typed_ptr = ptr.cast::<PoolItem<GcBox<T>>>();
                (*typed_ptr.as_ptr()).0.value.finalize();
                core::ptr::drop_in_place(typed_ptr.as_ptr());
                pool.free_slot(ptr);
            }
        }

        let mut pool = self.pool.borrow_mut();
        let ptr = pool.try_alloc(GcBox::new(value, drop_and_free::<T>))?;

        drop(pool);

        Ok(Gc {
            ptr: unsafe { ptr.extend_lifetime() },
            _marker: PhantomData,
        })
    }

    /// Runs a collection cycle (no-op for null collector)
    pub(crate) fn collect(&self) {}
}

impl Drop for Collector {
    /// Frees all remaining allocations
    fn drop(&mut self) {
        use crate::alloc::mempool3::PoolItem;

        // Free all GC allocations
        let all: Vec<(NonNull<u8>, DropFn)> = self
            .pool
            .borrow()
            .iter_live_slots()
            .map(|ptr| unsafe {
                let drop_fn = (*ptr.cast::<PoolItem<GcBox<()>>>().as_ptr()).0.drop_fn;
                (ptr, drop_fn)
            })
            .collect();
        let mut pool = self.pool.borrow_mut();
        for (ptr, drop_fn) in all {
            unsafe {
                (drop_fn)(&mut pool, ptr);
            }
        }
    }
}

/// Owns the GC and carries the `'id` context brand
pub struct GcContext<'id> {
    collector: Collector,
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id> GcContext<'id> {
    /// Opens a mutation window and passes a [`MutationContext`] to `f`
    /// Triggers a gc cycle
    pub fn collect(&self) {
        self.collector.collect();
    }

    pub fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'id, 'gc>) -> R) -> R {
        let cx = MutationContext {
            collector: &self.collector,
            _marker: PhantomData,
        };
        f(&cx)
    }
}

/// Create new GC context
pub fn with_gc<R, F: for<'id> FnOnce(GcContext<'id>) -> R>(f: F) -> R {
    f(GcContext {
        collector: Collector::new(),
        _marker: PhantomData,
    })
}
