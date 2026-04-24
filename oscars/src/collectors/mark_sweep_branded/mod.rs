//! Lifetime-branded mark and sweep garbage collector
#![cfg_attr(not(any(test, feature = "std")), allow(unused_imports))]

pub mod cell;
pub mod ephemeron;
pub mod gc;
pub mod gc_box;
pub mod mutation_ctx;
pub mod root_link;
pub mod trace;
pub mod weak;

#[cfg(all(test, feature = "mark_sweep_branded"))]
mod tests;

pub use cell::GcRefCell;
pub use ephemeron::Ephemeron;
pub use gc::{Gc, Root};
pub use mutation_ctx::MutationContext;
pub use trace::{Finalize, Trace, TraceColor};
pub use weak::WeakGc;

use crate::alloc::mempool3::PoolAllocator;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::mem;
use core::ptr::NonNull;
use gc_box::{DropFn, GcBox};
use root_link::{RootLink, RootSentinel};
use rust_alloc::vec::Vec;

/// Type-erased ephemeron registration.
pub(crate) struct EphemeronEntry {
    pub(crate) key_ptr: NonNull<u8>,
    pub(crate) key_alloc_id: usize,
    pub(crate) value_ptr: NonNull<u8>,
}

pub(crate) struct Collector {
    // SAFETY: We use 'static here because the PoolAllocator owns its memory,
    // and we ensure that `Gc` objects and pool allocations do not outlive
    // the `Collector` instance
    pub(crate) pool: RefCell<PoolAllocator<'static>>,
    /// Dedicated pool for RootNode allocations
    pub(crate) root_pool: RefCell<PoolAllocator<'static>>,
    pub(crate) sentinel: RootSentinel,
    pub(crate) generic_alloc_id: Cell<usize>,
    pub(crate) ephemerons: RefCell<Vec<EphemeronEntry>>,
}

impl Collector {
    fn new() -> Self {
        Self {
            pool: RefCell::new(PoolAllocator::default()),
            root_pool: RefCell::new(PoolAllocator::default()),
            sentinel: RootSentinel::new(),
            generic_alloc_id: Cell::new(0),
            ephemerons: RefCell::new(Vec::new()),
        }
    }

    /// Registers an ephemeron key/value pair for processing during collection.
    pub(crate) fn register_ephemeron(
        &self,
        key_ptr: NonNull<u8>,
        key_alloc_id: usize,
        value_ptr: NonNull<u8>,
    ) {
        self.ephemerons.borrow_mut().push(EphemeronEntry {
            key_ptr,
            key_alloc_id,
            value_ptr,
        });
    }

    /// Allocates a RootNode from the dedicated root pool.
    pub(crate) fn alloc_root_node<'id, T: trace::Trace>(
        &self,
        gc_ptr: NonNull<crate::alloc::mempool3::PoolItem<GcBox<T>>>,
    ) -> NonNull<gc::RootNode<'id, T>> {
        use crate::alloc::mempool3::PoolItem;

        unsafe fn drop_and_free<T: trace::Trace>(
            pool: &mut PoolAllocator<'static>,
            ptr: NonNull<u8>,
        ) {
            unsafe {
                let typed_ptr = ptr.cast::<PoolItem<gc::RootNode<'_, T>>>();
                core::ptr::drop_in_place(typed_ptr.as_ptr());
                pool.free_slot(ptr);
            }
        }

        let size = mem::size_of::<PoolItem<gc::RootNode<'id, T>>>();
        let mut pool = self.root_pool.borrow_mut();
        let slot_ptr = pool
            .alloc_slot_raw(size)
            .expect("root pool allocation failed");

        unsafe {
            let typed_ptr = slot_ptr.cast::<PoolItem<gc::RootNode<'id, T>>>();
            core::ptr::write(
                typed_ptr.as_ptr(),
                PoolItem(gc::RootNode {
                    link: root_link::RootLink::new(),
                    gc_ptr,
                    drop_fn: drop_and_free::<T>,
                    collector_ptr: self as *const Collector,
                    _marker: PhantomData,
                }),
            );
            NonNull::new_unchecked(&mut (*typed_ptr.as_ptr()).0)
        }
    }

    /// Frees a RootNode back to the root pool.
    pub(crate) fn free_root_node(&self, ptr: NonNull<u8>, drop_fn: gc::RootDropFn) {
        let mut pool = self.root_pool.borrow_mut();
        unsafe {
            (drop_fn)(&mut pool, ptr);
        }
    }

    /// Allocates a value from the pool.
    ///
    /// # Panics
    ///
    /// Panics if the allocation ID counter wraps around to `FREED_ALLOC_ID`
    /// This is a theoretical limit that would require `usize::MAX - 1` allocations.
    pub(crate) fn alloc<'gc, T: trace::Trace + trace::Finalize + 'gc>(
        &'gc self,
        value: T,
    ) -> Gc<'gc, T> {
        let alloc_id = self.generic_alloc_id.get();

        // Check for alloc_id wrap before incrementing.
        // If alloc_id reaches FREED_ALLOC_ID (usize::MAX), weak reference validation
        // would break because freed slots are marked with this sentinel value.
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
            color: &crate::collectors::mark_sweep_branded::trace::TraceColor<'_>,
        ) {
            use crate::alloc::mempool3::PoolItem;
            let pool_item_ptr = ptr.cast::<PoolItem<GcBox<T>>>();
            unsafe {
                (*pool_item_ptr.as_ptr()).0.value.trace(color);
            }
        }

        // Allocate a raw slot for PoolItem<GcBox<T>>
        let size = mem::size_of::<crate::alloc::mempool3::PoolItem<GcBox<T>>>();

        let mut pool = self.pool.borrow_mut();
        let slot_ptr = pool
            .alloc_slot_raw(size)
            .expect("branded GC: pool allocation failed");

        // SAFETY: slot_ptr points to uninitialized memory of the correct size and alignment.
        // We initialize it here before releasing the borrow.
        let ptr = unsafe {
            use crate::alloc::mempool3::PoolItem;
            let pool_item_ptr = slot_ptr.cast::<PoolItem<GcBox<T>>>();

            // Initialize the PoolItem<GcBox<T>> in place
            core::ptr::write(
                pool_item_ptr.as_ptr(),
                PoolItem(GcBox {
                    marked: Cell::new(false),
                    trace_fn: trace_value::<T>,
                    drop_fn: drop_and_free::<T>,
                    alloc_id,
                    value,
                }),
            );

            pool_item_ptr
        };

        drop(pool);

        unsafe fn drop_and_free<T: trace::Trace + trace::Finalize>(
            pool: &mut PoolAllocator<'static>,
            ptr: NonNull<u8>,
        ) {
            use crate::alloc::mempool3::PoolItem;
            unsafe {
                let typed_ptr = ptr.cast::<PoolItem<GcBox<T>>>();
                // Finalize the value
                (*typed_ptr.as_ptr()).0.value.finalize();
                // Drop the PoolItem<GcBox<T>> in place
                core::ptr::drop_in_place(typed_ptr.as_ptr());
                // Return the slot to the pool
                pool.free_slot(ptr);
            }
        }

        Gc {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Runs a collection cycle
    pub(crate) fn collect(&self) {
        let color = TraceColor::new();

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

        for link_ptr in self.sentinel.iter() {
            unsafe {
                // Read the `gc_ptr` field using the stable offset
                let gc_ptr_ptr = link_ptr
                    .as_ptr()
                    .cast::<u8>()
                    .add(gc_ptr_offset)
                    .cast::<NonNull<u8>>();
                let gc_ptr = gc_ptr_ptr.read();

                color.mark_raw(gc_ptr.cast::<u8>());
            }
        }

        color.drain();

        // Phase 2: ephemeron fixpoint.
        // If marking a value causes new keys of other ephemerons to become
        // reachable, we must iterate until no further values are marked.
        loop {
            let mut any_newly_marked = false;
            for entry in self.ephemerons.borrow().iter() {
                use crate::alloc::mempool3::PoolItem;
                unsafe {
                    let key_box = entry.key_ptr.cast::<PoolItem<GcBox<()>>>();
                    // Skip entries invalidated by a previous collection cycle.
                    if (*key_box.as_ptr()).0.alloc_id != entry.key_alloc_id {
                        continue;
                    }
                    if (*key_box.as_ptr()).0.marked.get() {
                        any_newly_marked |= color.mark_raw(entry.value_ptr);
                    }
                }
            }
            if !any_newly_marked {
                break;
            }
            color.drain();
        }

        // Phase 3: sweep all slots. Collect unmarked ones, then invalidate and free them.
        use crate::alloc::mempool3::PoolItem;
        let dead: Vec<(NonNull<u8>, DropFn)> = {
            let pool = self.pool.borrow();
            pool.iter_live_slots()
                .filter_map(|ptr| unsafe {
                    let gc_box = &(*ptr.cast::<PoolItem<GcBox<()>>>().as_ptr()).0;
                    if gc_box.marked.get() {
                        gc_box.marked.set(false);
                        None
                    } else {
                        Some((ptr, gc_box.drop_fn))
                    }
                })
                .collect()
        };
        {
            let mut pool = self.pool.borrow_mut();
            for (ptr, drop_fn) in dead {
                unsafe {
                    (*ptr.cast::<PoolItem<GcBox<()>>>().as_ptr()).0.alloc_id =
                        GcBox::<()>::FREED_ALLOC_ID;
                    (drop_fn)(&mut pool, ptr);
                }
            }
        }

        // Phase 4: remove ephemeron entries whose key was swept this cycle.
        self.ephemerons.borrow_mut().retain(|entry| {
            use crate::alloc::mempool3::PoolItem;
            unsafe {
                let key_box = entry.key_ptr.cast::<PoolItem<GcBox<()>>>();
                (*key_box.as_ptr()).0.alloc_id == entry.key_alloc_id
            }
        });
    }
}

impl Drop for Collector {
    /// Frees all remaining allocations
    fn drop(&mut self) {
        use crate::alloc::mempool3::PoolItem;

        // Free all root nodes first
        let all_roots: Vec<(NonNull<u8>, gc::RootDropFn)> = self
            .root_pool
            .borrow()
            .iter_live_slots()
            .map(|ptr| unsafe {
                let drop_fn = (*ptr.cast::<PoolItem<gc::RootNode<'_, ()>>>().as_ptr())
                    .0
                    .drop_fn;
                (ptr, drop_fn)
            })
            .collect();
        let mut root_pool = self.root_pool.borrow_mut();
        for (ptr, drop_fn) in all_roots {
            unsafe {
                (drop_fn)(&mut root_pool, ptr);
            }
        }
        drop(root_pool);

        // Then free all GC allocations
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
                (*ptr.cast::<PoolItem<GcBox<()>>>().as_ptr()).0.alloc_id =
                    GcBox::<()>::FREED_ALLOC_ID;
                (drop_fn)(&mut pool, ptr);
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

    #[cfg(test)]
    pub(crate) fn ephemeron_count(&self) -> usize {
        self.collector.ephemerons.borrow().len()
    }
}

/// Creates a new GC context.
pub fn with_gc<R, F: for<'id> FnOnce(GcContext<'id>) -> R>(f: F) -> R {
    f(GcContext {
        collector: Collector::new(),
        _marker: PhantomData,
    })
}
