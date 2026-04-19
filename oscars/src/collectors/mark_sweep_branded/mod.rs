//! Lifetime-branded mark and sweep garbage collector
#![cfg_attr(not(any(test, feature = "std")), allow(unused_imports))]

pub mod cell;
pub mod gc;
pub mod gc_box;
pub mod mutation_ctx;
pub mod root_link;
pub mod trace;
pub mod weak;

#[cfg(all(test, feature = "mark_sweep_branded"))]
mod tests;

pub use cell::GcRefCell;
pub use gc::{Gc, Root};
pub use mutation_ctx::MutationContext;
pub use trace::{Finalize, Trace, Tracer};
pub use weak::WeakGc;

use crate::alloc::mempool3::PoolAllocator;
use core::alloc::Layout;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::pin::Pin;
use core::ptr::NonNull;
use gc_box::GcBox;
use root_link::RootLink;
use rust_alloc::boxed::Box;
use rust_alloc::vec::Vec;

/// Erased drop-fn
struct PoolEntry {
    ptr: NonNull<u8>,
    drop_fn: unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>),
}

pub(crate) struct Collector {
    // SAFETY: We use 'static here because the PoolAllocator owns its memory,
    // and we ensure that `Gc` objects and pool allocations do not outlive
    // the `Collector` instance.
    pub(crate) pool: RefCell<PoolAllocator<'static>>,
    pool_entries: RefCell<Vec<PoolEntry>>,
    pub(crate) sentinel: Pin<Box<RootLink>>,
    allocation_count: Cell<usize>,
    pub(crate) generic_alloc_id: Cell<usize>,
}

impl Collector {
    fn new() -> Self {
        Self {
            pool: RefCell::new(PoolAllocator::default()),
            pool_entries: RefCell::new(Vec::new()),
            sentinel: Box::pin(RootLink::new()),
            allocation_count: Cell::new(0),
            generic_alloc_id: Cell::new(0),
        }
    }

    /// Allocates a value from the pool.
    ///
    /// # Panics
    ///
    /// Panics if the allocation ID counter wraps around to `FREED_ALLOC_ID`.
    /// This is a theoretical limit that would require `usize::MAX - 1` allocations.
    pub(crate) fn alloc<'gc, T: trace::Trace + trace::Finalize + 'gc>(
        &'gc self,
        value: T,
    ) -> Gc<'gc, T> {
        let alloc_id = self.generic_alloc_id.get();

        // Check for alloc_id wrap before incrementing.
        // If alloc_id reaches FREED_ALLOC_ID (usize::MAX), weak reference validation
        // would break because freed slots are marked with this sentinel value.
        // This is a theoretical limit requiring usize::MAX - 1 allocations.
        assert_ne!(
            alloc_id,
            GcBox::<()>::FREED_ALLOC_ID,
            "Allocation ID counter wrapped to FREED_ALLOC_ID sentinel. \
             This indicates usize::MAX - 1 allocations have been made, \
             which would break weak reference ABA protection."
        );

        self.generic_alloc_id.set(alloc_id.wrapping_add(1));

        unsafe fn trace_value<T: trace::Trace>(
            ptr: core::ptr::NonNull<u8>,
            tracer: &mut crate::collectors::mark_sweep_branded::trace::Tracer<'_>,
        ) {
            let gcbox_ptr = ptr.cast::<GcBox<T>>();
            unsafe {
                (*gcbox_ptr.as_ptr()).value.trace(tracer);
            }
        }

        let gcbox = GcBox {
            marked: Cell::new(false),
            trace_fn: Some(trace_value::<T>),
            alloc_id,
            value,
        };

        let layout = Layout::new::<GcBox<T>>();
        let slot = self
            .pool
            .borrow_mut()
            .try_alloc_bytes(layout)
            .expect("branded GC: pool allocation failed");

        // SAFETY: `slot` has the correct layout and alignment for `GcBox<T>`.
        let ptr = unsafe {
            let ptr = slot.cast::<GcBox<T>>();
            ptr.as_ptr().write(gcbox);
            ptr
        };

        unsafe fn drop_and_free<T: trace::Trace + trace::Finalize>(
            pool: &mut PoolAllocator<'static>,
            ptr: NonNull<u8>,
        ) {
            unsafe {
                ptr.cast::<GcBox<T>>().as_ref().value.finalize();
                core::ptr::drop_in_place(ptr.cast::<GcBox<T>>().as_ptr());
                pool.dealloc_bytes(ptr);
            }
        }

        self.pool_entries.borrow_mut().push(PoolEntry {
            ptr: ptr.cast::<u8>(),
            drop_fn: drop_and_free::<T>,
        });

        self.allocation_count.set(self.allocation_count.get() + 1);

        Gc {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Runs a collection cycle
    pub(crate) fn collect(&self) {
        // SAFETY: sentinel is `Pin<Box<RootLink>>`.
        let sentinel_ptr = unsafe {
            NonNull::new_unchecked(
                self.sentinel.as_ref().get_ref() as *const RootLink as *mut RootLink
            )
        };

        let mut tracer = Tracer::new();

        let gc_ptr_offset = core::mem::offset_of!(
            crate::collectors::mark_sweep_branded::gc::RootNode<'static, i32>,
            gc_ptr
        );
        debug_assert_eq!(
            gc_ptr_offset,
            core::mem::offset_of!(
                crate::collectors::mark_sweep_branded::gc::RootNode<'static, u64>,
                gc_ptr
            ),
            "gc_ptr offset must be stable across all T: Sized"
        );

        for link_ptr in RootLink::iter_from_sentinel(sentinel_ptr) {
            unsafe {
                // Read the `gc_ptr` field using the stable offset
                let gc_ptr_ptr = link_ptr
                    .as_ptr()
                    .cast::<u8>()
                    .add(gc_ptr_offset)
                    .cast::<NonNull<u8>>();
                let gc_ptr = gc_ptr_ptr.read();

                tracer.mark_raw(gc_ptr.cast::<u8>());
            }
        }

        tracer.drain();

        let mut pool = self.pool.borrow_mut();
        self.pool_entries.borrow_mut().retain_mut(|entry| {
            // SAFETY: `ptr` was written with a valid `GcBox<T>`.
            let marked = unsafe {
                let gcbox = entry.ptr.as_ptr() as *mut GcBox<()>;
                (*gcbox).marked.get()
            };

            if marked {
                unsafe {
                    let gcbox = entry.ptr.as_ptr() as *mut GcBox<()>;
                    (*gcbox).marked.set(false);
                }
                true
            } else {
                unsafe {
                    let gcbox = entry.ptr.as_ptr() as *mut GcBox<()>;
                    (*gcbox).alloc_id =
                        crate::collectors::mark_sweep_branded::gc_box::GcBox::<()>::FREED_ALLOC_ID;
                    (entry.drop_fn)(&mut pool, entry.ptr);
                }
                false
            }
        });
    }
}

impl Drop for Collector {
    /// Frees all remaining allocations
    fn drop(&mut self) {
        let mut pool = self.pool.borrow_mut();
        for entry in self.pool_entries.borrow().iter() {
            unsafe {
                let gcbox = entry.ptr.as_ptr() as *mut GcBox<()>;
                (*gcbox).alloc_id =
                    crate::collectors::mark_sweep_branded::gc_box::GcBox::<()>::FREED_ALLOC_ID;
                (entry.drop_fn)(&mut pool, entry.ptr);
            }
        }
    }
}

/// Owns the garbage collector and carries the `'id` context brand
pub struct GcContext<'id> {
    collector: Collector,
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id> GcContext<'id> {
    /// Opens a mutation window and passes a [`MutationContext`] to `f`.
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

/// Creates a new GC context.
pub fn with_gc<R, F: for<'id> FnOnce(GcContext<'id>) -> R>(f: F) -> R {
    f(GcContext {
        collector: Collector::new(),
        _marker: PhantomData,
    })
}
