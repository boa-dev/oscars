//! Lifetime-branded mark and sweep garbage collector
#![cfg_attr(not(any(test, feature = "std")), allow(unused_imports))]

pub mod cell;
pub mod ephemeron;
pub mod gc;
pub mod gc_box;
pub mod mutation_ctx;
pub mod root;
pub mod trace;
pub mod weak;

#[cfg(all(test, feature = "mark_sweep_branded"))]
mod tests;

pub use cell::GcRefCell;
pub use ephemeron::Ephemeron;
pub use gc::Gc;
pub use mutation_ctx::MutationContext;
pub use root::Root;
pub use trace::{Finalize, Trace, Tracer};
pub use weak::WeakGc;

use crate::alloc::mempool3::{PoolAllocError, PoolAllocator, PoolPointer};
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::ptr::NonNull;
use gc_box::{DropFn, GcBox, GcColor};
use root::RootSentinel;
use rust_alloc::vec::Vec;

/// Type-erased ephemeron registration.
pub(crate) struct EphemeronEntry {
    pub(crate) key_ptr: Option<PoolPointer<'static, GcBox<()>>>,
    pub(crate) value_ptr: PoolPointer<'static, GcBox<()>>,
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
        key_ptr: PoolPointer<'static, GcBox<()>>,
        value_ptr: PoolPointer<'static, GcBox<()>>,
    ) {
        self.ephemerons.borrow_mut().push(EphemeronEntry {
            key_ptr: Some(key_ptr),
            value_ptr,
        });
    }

    /// Allocates a RootNode from the dedicated root pool and links it into the root list.
    pub(crate) fn try_alloc_root_node<'id, T: trace::Trace>(
        &self,
        gc_ptr: PoolPointer<'static, GcBox<T>>,
    ) -> Result<NonNull<root::RootNode<'id, T>>, PoolAllocError> {
        let mut pool = self.root_pool.borrow_mut();
        let ptr = pool.try_alloc(root::RootNode::new_in(gc_ptr, self))?;
        // SAFETY: PoolItem<T> is repr(transparent) over T; pointer address is identical.
        let raw = ptr.as_ptr().cast::<root::RootNode<'id, T>>();
        // SAFETY: `raw` points to a stable `RootNode` allocated in the pool.
        unsafe {
            root::RootLink::link_after(self.sentinel.as_ptr(), raw.cast::<root::RootLink>());
        }
        Ok(raw)
    }

    /// Frees a RootNode back to the root pool.
    pub(crate) fn free_root_node(&self, ptr: NonNull<u8>, drop_fn: root::RootDropFn) {
        let mut pool = self.root_pool.borrow_mut();
        unsafe {
            (drop_fn)(&mut pool, ptr);
        }
    }

    /// Allocates a value from the pool.
    ///
    /// # Errors
    ///
    /// Returns `Err(PoolAllocError::AllocIdExhausted)` if the allocation ID counter
    /// has reached `FREED_ALLOC_ID` (`usize::MAX`). This is a theoretical limit
    /// that would require `usize::MAX - 1` allocations.
    pub(crate) fn try_alloc<'gc, T: trace::Trace + trace::Finalize + 'gc>(
        &'gc self,
        value: T,
    ) -> Result<Gc<'gc, T>, PoolAllocError> {
        let alloc_id = self.generic_alloc_id.get();

        // Check for alloc_id wrap before incrementing.
        // If alloc_id reaches FREED_ALLOC_ID (usize::MAX), weak reference validation
        // would break because freed slots are marked with this sentinel value.
        if alloc_id == GcBox::<()>::FREED_ALLOC_ID {
            return Err(PoolAllocError::AllocIdExhausted);
        }

        self.generic_alloc_id.set(alloc_id.wrapping_add(1));

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
        let ptr = pool.try_alloc(GcBox::new(
            value,
            gc_box::trace_value::<T>,
            drop_and_free::<T>,
            alloc_id,
        ))?;

        drop(pool);

        Ok(Gc::with_pointer(unsafe { ptr.extend_lifetime() }))
    }

    /// Runs a collection cycle
    pub(crate) fn collect(&self) {
        let mut tracer = Tracer::new();

        for link_ptr in self.sentinel.iter() {
            unsafe {
                // SAFETY: link_ptr points to the `link` field which is first in repr(C) RootNode.
                // Casting to ErasedRootNode (also repr(C), same first two fields) lets us read
                // gc_ptr without knowing T, avoiding manual offset arithmetic.
                let erased = link_ptr.cast::<root::ErasedRootNode>();
                tracer.mark_raw((*erased.as_ptr()).gc_ptr.as_ptr().cast::<u8>());
            }
        }

        tracer.drain();

        // Phase 2: ephemeron fixpoint.
        // If marking a value causes new keys of other ephemerons to become
        // reachable, we must iterate until no further values are marked.
        loop {
            let mut any_newly_marked = false;
            for entry in self.ephemerons.borrow().iter() {
                let Some(key_ptr) = entry.key_ptr else {
                    continue;
                };
                unsafe {
                    if (*key_ptr.as_ptr().as_ptr()).0.color.get() != GcColor::White {
                        any_newly_marked |= tracer.mark_raw(entry.value_ptr.as_ptr().cast::<u8>());
                    }
                }
            }
            if !any_newly_marked {
                break;
            }
            tracer.drain();
        }

        // Phase 3: sweep all slots. Collect unmarked ones, then invalidate and free them.
        use crate::alloc::mempool3::PoolItem;
        let dead: Vec<(NonNull<u8>, DropFn)> = {
            let pool = self.pool.borrow();
            pool.iter_live_slots()
                .filter_map(|ptr| unsafe {
                    let gc_box = &(*ptr.cast::<PoolItem<GcBox<()>>>().as_ptr()).0;
                    if gc_box.color.get() == GcColor::Black {
                        gc_box.color.set(GcColor::White);
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
        // A swept key has alloc_id set to FREED_ALLOC_ID by the sweep above.
        // Using the Option lets us express the invalid state without a stored alloc_id.
        self.ephemerons.borrow_mut().retain(|entry| {
            entry.key_ptr.is_some_and(|key_ptr| unsafe {
                (*key_ptr.as_ptr().as_ptr()).0.alloc_id != GcBox::<()>::FREED_ALLOC_ID
            })
        });
    }
}

impl Drop for Collector {
    /// Frees all remaining allocations
    fn drop(&mut self) {
        use crate::alloc::mempool3::PoolItem;

        // Free all root nodes first
        let all_roots: Vec<(NonNull<u8>, root::RootDropFn)> = self
            .root_pool
            .borrow()
            .iter_live_slots()
            .map(|ptr| unsafe {
                let drop_fn = (*ptr.cast::<PoolItem<root::RootNode<'_, ()>>>().as_ptr())
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
