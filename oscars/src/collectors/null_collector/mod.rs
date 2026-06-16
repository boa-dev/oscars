//! A null (no-op) GC
//!
//! [`NullCollector`] uses the same arena as [`crate::collectors::mark_sweep::MarkSweepGarbageCollector`]
//! but never collects. Allocations are only freed when the collector drops.
//!
//! # Use Cases
//! * **Short-lived contexts**: Avoids GC overhead when the heap is discarded quickly.
//! * **Benchmarking**: Measures raw allocation costs without GC interference.
//!
//! # Limitations
//! * **No cycle collection**: Leaks memory in long running programs.
//! * **Weak pointers stay alive**: `WeakGc::upgrade` always succeeds.

use core::cell::RefCell;
use core::ptr::NonNull;

use crate::{
    alloc::mempool3::{PoolAllocError, PoolAllocator, PoolItem, PoolPointer},
    collectors::mark_sweep::{
        Collector, ErasedEphemeron, ErasedWeakMap, Gc, TraceColor,
        internals::{Ephemeron, GcBox, NonTraceable},
        trace::Trace,
    },
};
use rust_alloc::vec::Vec;

#[cfg(test)]
mod tests;

/// Fixed trace color.
/// We never sweep, so objects always stay the same color.
const NULL_TRACE_COLOR: TraceColor = TraceColor::White;

/// Type-erased root pointer.
/// Matches `MarkSweepGarbageCollector` to reuse vtable functions.
type GcErasedPointer = NonNull<PoolItem<GcBox<NonTraceable>>>;

/// A garbage collector that **never collects**.
///
/// Objects are allocated into an arena and tracked. Their destructors
/// run when the collector is dropped. No mark or sweep passes happen
/// during normal execution.
pub struct NullCollector {
    /// Backing pool allocator for accurate benchmarking.
    pub(crate) allocator: RefCell<PoolAllocator<'static>>,

    /// All `GcBox` nodes in insertion order.
    /// Used during drop to run finalizers and destructors.
    root_queue: RefCell<Vec<GcErasedPointer>>,

    /// All `Ephemeron` nodes in insertion order.
    ephemeron_queue: RefCell<Vec<ErasedEphemeron>>,

    /// Heap allocations for `WeakMapInner`.
    /// Tracked to allow safe drops and freed when the collector drops.
    weak_maps: RefCell<Vec<NonNull<dyn ErasedWeakMap>>>,
}

impl Default for NullCollector {
    fn default() -> Self {
        Self {
            allocator: RefCell::new(PoolAllocator::default()),
            root_queue: RefCell::new(Vec::new()),
            ephemeron_queue: RefCell::new(Vec::new()),
            weak_maps: RefCell::new(Vec::new()),
        }
    }
}

impl NullCollector {
    /// Override the page size used by the underlying allocator.
    ///
    /// This is useful in tests and matches the `MarkSweepGarbageCollector` API.
    #[must_use]
    pub fn with_page_size(mut self, page_size: usize) -> Self {
        self.allocator.get_mut().page_size = page_size;
        self
    }

    /// Override the heap threshold.
    ///
    /// The null collector never auto-collects, so this value is ignored.
    /// It exists to match the `MarkSweepGarbageCollector` constructor exactly.
    #[must_use]
    pub fn with_heap_threshold(mut self, heap_threshold: usize) -> Self {
        self.allocator.get_mut().heap_threshold = heap_threshold;
        self
    }

    /// Number of live slot-pool pages and bump pages.
    ///
    /// This mirrors `MarkSweepGarbageCollector::pools_len` for testing.
    pub fn pools_len(&self) -> usize {
        self.allocator.borrow().pools_len()
    }
}

impl NullCollector {
    /// Finalize and free all tracked nodes.
    ///
    /// This uses two phases so finalizers can safely access other GC values
    /// that are still in the heap:
    ///
    /// * Phase 1: call `finalize_fn` for all roots and ephemerons.
    /// * Phase 2: call `drop_fn` for all roots and ephemerons, then
    ///   free the slots.
    ///
    /// This matches `MarkSweepGarbageCollector::sweep_all_queues`.
    fn sweep_all_queues(&self) {
        let roots = core::mem::take(&mut *self.root_queue.borrow_mut());
        let ephemerons = core::mem::take(&mut *self.ephemeron_queue.borrow_mut());

        // Phase 1: finalize
        for node in roots.iter().copied() {
            // SAFETY: `node` is a live pool allocation with a valid vtable.
            let gc_box = unsafe { node.as_ref().value() };
            unsafe { gc_box.finalize_fn()(node) };
        }

        for eph in ephemerons.iter().copied() {
            // SAFETY: `eph` is a live pool allocation with a valid vtable.
            let vtable = unsafe { eph.as_ref().value() };
            unsafe { vtable.finalize_fn()(eph) };
        }

        // Phase 2: drop + free
        for node in roots {
            // SAFETY: `drop_fn` is called exactly once before freeing the slot.
            let drop_fn = unsafe { node.as_ref().value().drop_fn() };
            unsafe { drop_fn(node) };
            self.allocator.borrow_mut().free_slot(node.cast::<u8>());
        }

        for eph in ephemerons {
            let drop_fn = unsafe { eph.as_ref().value().drop_fn() };
            unsafe { drop_fn(eph) };
            self.allocator.borrow_mut().free_slot(eph.cast::<u8>());
        }
    }

    /// Free `Box<dyn ErasedWeakMap>` allocations from `track_weak_map`.
    ///
    /// These pointers come from `Box::into_raw` and must be rebuilt into
    /// a `Box` to free them correctly.
    fn drop_weak_maps(&self) {
        for map_ptr in self.weak_maps.borrow_mut().drain(..) {
            // SAFETY: `map_ptr` came from `Box::into_raw` in `WeakMap::new`.
            unsafe {
                let _ = rust_alloc::boxed::Box::from_raw(map_ptr.as_ptr());
            }
        }
    }
}

impl Drop for NullCollector {
    fn drop(&mut self) {
        // If any rooted handles outlive the collector, skip teardown to
        // avoid use-after-free. The pool pages will be freed by the allocator.
        // This matches `MarkSweepGarbageCollector::drop`.
        let has_rooted = self
            .root_queue
            .borrow()
            .iter()
            .any(|node| unsafe { node.as_ref().value().is_rooted() });

        if self.pools_len() > 0 && has_rooted {
            // Intentional leak: rooted handles outlive the collector.
        } else {
            self.sweep_all_queues();
        }

        self.drop_weak_maps();
    }
}

impl Collector for NullCollector {
    /// No-op: the null collector never triggers a collection cycle.
    ///
    /// Calling `collect` does nothing, regardless of heap size or pressure.
    #[inline]
    fn collect(&self) {}

    /// Returns the fixed trace-color epoch.
    ///
    /// The null collector never flips the epoch. We always return a constant
    /// color since we don't use it for sweeping.
    #[inline]
    fn gc_color(&self) -> TraceColor {
        NULL_TRACE_COLOR
    }

    /// Allocate a `GcBox<T>` and register it for teardown.
    ///
    /// Unlike `MarkSweepGarbageCollector`, this never triggers collections.
    /// The node goes on the root queue for finalization when the collector drops.
    ///
    /// The lifetime `'gc` ties the returned pointer to `self`, ensuring the
    /// pointer cannot outlive the pool that backs it.
    fn alloc_gc_node<'gc, T: Trace + 'static>(
        &'gc self,
        value: T,
    ) -> Result<PoolPointer<'gc, GcBox<T>>, PoolAllocError> {
        let gc_box = GcBox::new_in(value, NULL_TRACE_COLOR);
        let arena_ptr = self.allocator.borrow_mut().try_alloc(gc_box)?;

        let erased: GcErasedPointer = arena_ptr.as_ptr().cast();
        self.root_queue.borrow_mut().push(erased);

        Ok(arena_ptr)
    }

    /// Allocate an `Ephemeron<K, V>` and register it for teardown.
    ///
    /// No collection is ever triggered. Because the collector never sweeps,
    /// the ephemeron key is never invalidated. `WeakGc::upgrade` always succeeds.
    fn alloc_ephemeron_node<'gc, K: Trace + 'static, V: Trace + 'static>(
        &'gc self,
        key: &Gc<K>,
        value: V,
    ) -> Result<PoolPointer<'gc, Ephemeron<K, V>>, PoolAllocError> {
        let ephemeron = Ephemeron::new(key, value, NULL_TRACE_COLOR);
        let inner_ptr = self.allocator.borrow_mut().try_alloc(ephemeron)?;

        let eph_ptr = inner_ptr
            .as_ptr()
            .cast::<PoolItem<Ephemeron<NonTraceable, NonTraceable>>>();
        self.ephemeron_queue.borrow_mut().push(eph_ptr);

        Ok(inner_ptr)
    }

    /// Register a `WeakMap` with the collector.
    ///
    /// We never prune dead entries, so weak map entries stay alive.
    /// We accept the registration so `WeakMap::drop` can mark itself dead
    /// without panicking. The memory is reclaimed in `drop_weak_maps`.
    #[doc(hidden)]
    #[inline]
    fn track_weak_map(&self, map: NonNull<dyn ErasedWeakMap>) {
        self.weak_maps.borrow_mut().push(map);
    }
}
