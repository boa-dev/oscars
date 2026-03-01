//! An implementation of a mark sweep collector
//!
//! This was initially a copy of `boa_gc` with alterations to make the collector
//! `no_std`

use core::cell::{Cell, RefCell};
use core::ptr::NonNull;

use crate::{
    alloc::arena3::{ArenaAllocator, ArenaHeapItem, ArenaPointer},
    collectors::mark_sweep::{
        internals::{Ephemeron, GcBox, NonTraceable},
        pointers::weak_map::ErasedWeakMap,
    },
};
use rust_alloc::vec::Vec;

mod pointers;
pub(crate) mod trace;

pub mod cell;

#[cfg(all(test, feature = "mark_sweep"))]
mod tests;

pub(crate) mod internals;

pub use pointers::weak_map::WeakMap;
pub use pointers::{Gc, Root, WeakGc};
pub use trace::{Finalize, Trace, TraceColor};

type GcErasedPointer = NonNull<ArenaHeapItem<GcBox<NonTraceable>>>;
pub(crate) type ErasedEphemeron = NonNull<ArenaHeapItem<Ephemeron<NonTraceable, NonTraceable>>>;

/* TODO: Figure out the best way to adapt the thread local concept in no_std
*
* NOTE: Maybe, the thread_local should be left up to the user or a std feature
*
* use core::cell::{RefCell, Cell};
*
* thread_local!(static GC_DROPPING: Cell<bool> = const { Cell::new(false) });
* thread_local!(static BOA_GC: RefCell<BoaGc> = RefCell::new( BoaGc {
*     config: GcConfig::default(),
*     runtime: GcRuntimeData::default(),
*     strongs: Vec::default(),
*     weaks: Vec::default(),
*     weak_maps: Vec::default(),
* }));
*/

#[derive(Default)]
pub struct MarkSweepGarbageCollector {
    // we use RefCell so we can borrow the arena mutably via &self
    // this fits the Allocator trait and is safe for single-threaded use
    pub(crate) allocator: RefCell<ArenaAllocator<'static>>,
    root_queue: RefCell<Vec<GcErasedPointer>>,
    ephemeron_queue: RefCell<Vec<ErasedEphemeron>>,
    // current trace color epoch, flips each cycle
    pub(crate) trace_color: Cell<TraceColor>,
    // true if the heap crossed its threshold, triggers a deferred collection
    collect_needed: Cell<bool>,
    // true during a collection, pushes new allocations to pending queues to prevent crashes
    is_collecting: Cell<bool>,
    pending_root_queue: RefCell<Vec<GcErasedPointer>>,
    pending_ephemeron_queue: RefCell<Vec<ErasedEphemeron>>,
    pub(crate) weak_maps: RefCell<Vec<NonNull<dyn ErasedWeakMap>>>,
}

impl MarkSweepGarbageCollector {
    pub fn with_heap_threshold(mut self, heap_threshold: usize) -> Self {
        self.allocator.get_mut().heap_threshold = heap_threshold;
        self
    }

    pub fn with_arena_size(mut self, arena_size: usize) -> Self {
        self.allocator.get_mut().arena_size = arena_size;
        self
    }

    //returns the number of live arenas held by this collector
    //
    //prefer this over accessing `self.allocator` directly in tests so that
    //the arena representation can change without touching every call site
    pub fn arenas_len(&self) -> usize {
        self.allocator.borrow().arenas_len()
    }
}

impl Drop for MarkSweepGarbageCollector {
    fn drop(&mut self) {
        // Reclaim all collector-owned weak maps.
        // Single-threaded, so this is safe.
        for &map_ptr in self.weak_maps.borrow().iter() {
            unsafe {
                let _ = rust_alloc::boxed::Box::from_raw(map_ptr.as_ptr());
            }
        }

        // SAFETY:
        // `Root<T>` pointers act as if they live forever (`'static`).
        // if the GC drops while they exist, we leak the memory to prevent a UAF
        if self.arenas_len() > 0
            && (!self.root_queue.borrow().is_empty()
                || !self.pending_root_queue.borrow().is_empty())
        {
            // Unrooted items are NOT swept here so they intentionally leak
            // instead of triggering a Use-After-Free.
            // The underlying arena pools WILL be dropped (and OS memory reclaimed)
            // when `self.allocator` is dropped at the end of this scope.
        } else {
            // No rooted items are alive. Sweep and clean up the remaining
            // cycles and loose allocations before the allocator natively drops.
            self.sweep_all_queues();
        }
    }
}

// ==== Collection methods ====

// RAII guard that clears `is_collecting` even if a Trace or Finalize impl panics
// without this, a panic inside run_mark_phase / run_sweep_phase would leave
// is_collecting == true forever, silently disabling the deferred collect
struct CollectingGuard<'a>(&'a Cell<bool>);

impl Drop for CollectingGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl MarkSweepGarbageCollector {
    // trigger a full collection cycle
    //
    // exposes `&self` to run without borrow conflicts when live collections exist
    pub fn collect(&self) {
        // lock the main queues so allocations buffer into pending queues
        // the guard resets is_collecting even if a Trace/Finalize impl panics
        self.is_collecting.set(true);
        let _guard = CollectingGuard(&self.is_collecting);

        self.run_mark_phase();

        // the sweep color is the color used to mark alive objects during this cycle
        let sweep_color = self.trace_color.get();

        // prune dead entries from each collector owned weak map before freeing
        // memory so we can still inspect the trace color on ephemerons;
        // use sweep_color since alive objects were marked with it.
        self.sweep_trace_color(sweep_color);

        // finally tell the allocator to reclaim raw OS memory
        // from arenas that are completely empty now
        self.allocator.borrow_mut().drop_dead_arenas();
    }

    // Force drops all elements in the internal tracking queues and clears
    // them without regard for reachability.
    fn sweep_all_queues(&self) {
        let ephemerons = core::mem::take(&mut *self.ephemeron_queue.borrow_mut());
        for mut ephemeron in ephemerons {
            let ephemeron_ref = unsafe { ephemeron.as_ref() };
            unsafe { ephemeron_ref.value().drop_fn()(ephemeron) };
            self.allocator
                .borrow_mut()
                .free_slot(ephemeron.cast::<u8>());
        }

        let roots = core::mem::take(&mut *self.root_queue.borrow_mut());
        for node in roots {
            let node_ref = unsafe { node.as_ref() };
            unsafe { node_ref.value().drop_fn()(node) };
            self.allocator.borrow_mut().free_slot(node.cast::<u8>());
        }

        let pending_e = core::mem::take(&mut *self.pending_ephemeron_queue.borrow_mut());
        for mut ephemeron in pending_e {
            let ephemeron_ref = unsafe { ephemeron.as_ref() };
            unsafe { ephemeron_ref.value().drop_fn()(ephemeron) };
            self.allocator
                .borrow_mut()
                .free_slot(ephemeron.cast::<u8>());
        }

        let pending_r = core::mem::take(&mut *self.pending_root_queue.borrow_mut());
        for node in pending_r {
            let node_ref = unsafe { node.as_ref() };
            unsafe { node_ref.value().drop_fn()(node) };
            self.allocator.borrow_mut().free_slot(node.cast::<u8>());
        }
    }

    // Extracts and sweeps items that are considered dead (different trace color).
    fn sweep_trace_color(&self, sweep_color: TraceColor) {
        // We use retain and manually drop deleted maps to satisfy Miri's
        // pointer provenance rules (avoiding Box's unique ownership).
        self.weak_maps.borrow_mut().retain(|&map_ptr| {
            // SAFETY: the pointer is valid as long as it's in this list.
            let map = unsafe { map_ptr.as_ref() };
            if map.is_alive() {
                // We need mut access to prune.
                unsafe { (&mut *map_ptr.as_ptr()).prune_dead_entries(sweep_color) };
                true
            } else {
                // WeakMap was dropped, reclaim the inner allocation.
                unsafe {
                    let _ = rust_alloc::boxed::Box::from_raw(map_ptr.as_ptr());
                }
                false
            }
        });

        self.run_sweep_phase();

        // flip the trace color epoch so newly allocated objects get the next color
        let new_color = sweep_color.flip();
        self.trace_color.set(new_color);

        // NOTE: It would actually be interesting to reuse the arenas that are dead rather
        // than drop the page and reallocate when a new page is needed ... TBD
        self.allocator.borrow_mut().drop_dead_arenas();

        // Drain pending queues while `is_collecting` is still true so that any
        // allocation triggered by `drop(_guard)` flushes to pending (not main)
        // queues, preserving insertion-order invariants for cache-friendly traversal.
        self.root_queue
            .borrow_mut()
            .append(&mut self.pending_root_queue.borrow_mut());
        self.ephemeron_queue
            .borrow_mut()
            .append(&mut self.pending_ephemeron_queue.borrow_mut());

        // guard drops here, setting is_collecting = false
    }

    pub fn run_mark_phase(&self) {
        let color = self.trace_color.get();
        // Run marks through the roots
        for heap_item in self.root_queue.borrow().iter() {
            let heap_item_ref = unsafe { heap_item.as_ref() };
            if heap_item_ref.value().is_rooted() {
                unsafe {
                    heap_item_ref.value().trace_fn()(*heap_item, color);
                }
            }
        }

        for ephemeron_heap_item in self.ephemeron_queue.borrow().iter() {
            let ephemeron_ref = unsafe { ephemeron_heap_item.as_ref() };
            let is_reachable =
                unsafe { ephemeron_ref.value().is_reachable_fn()(*ephemeron_heap_item, color) };

            if is_reachable {
                // Mark the ephemeron itself in the arena bitmap so it isn't
                // reclaimed by drop_dead_arenas. Ephemerons don't have GcHeaders,
                // so we mark them manually.
                self.allocator
                    .borrow()
                    .mark_slot(ephemeron_heap_item.cast());

                unsafe { ephemeron_ref.value().trace_fn()(*ephemeron_heap_item, color) }
            }
        }

        // At this point, all objects should be marked.
    }

    pub fn run_sweep_phase(&self) {
        let color = self.trace_color.get();

        // NOTE: it is important here to only extract after attempting to finalize, this is
        // so that our queues ideally maintain the insertion order for so that they are cache
        // friendly.
        let droppables = self
            .root_queue
            .borrow_mut()
            .extract_if(.., |node| {
                let heap_item_ref = unsafe { node.as_ref() };
                let gc_box = heap_item_ref.value();
                // Check if the value is not reachable, i.e. dead.
                if !gc_box.is_reachable(color) {
                    // Finalize the dead item
                    gc_box.finalize();
                    // Recheck if the value is now rooted again after finalization.
                    if gc_box.is_rooted() {
                        unsafe { gc_box.trace_fn()(*node, color) };
                    }
                }
                // Extract if the value is still no longer reachable.
                !heap_item_ref.value().is_reachable(color)
            })
            .collect::<Vec<_>>();

        let ephemerons = self
            .ephemeron_queue
            .borrow_mut()
            .extract_if(.., |node| {
                let ephemeron_ref = unsafe { node.as_ref() };
                let vtable = ephemeron_ref.value();

                let is_reachable = unsafe { vtable.is_reachable_fn()(*node, color) };
                if !is_reachable {
                    unsafe { vtable.finalize_fn()(*node) };
                    // Recheck after finalization
                    if unsafe { vtable.is_reachable_fn()(*node, color) } {
                        unsafe { vtable.trace_fn()(*node, color) };
                    }
                }

                // Check whether the ephemeron is reachable.
                // An inactive ephemeron should be dropped.
                !unsafe { vtable.is_reachable_fn()(*node, color) }
            })
            .collect::<Vec<_>>();

        let mut still_alive_roots = Vec::default();

        let mut still_alive = Vec::default();
        for mut ephemeron in ephemerons {
            let ephemeron_ref = unsafe { ephemeron.as_ref() };
            // If it's reachable according to the color, and it's active
            // (both are checked inside the vtable-dispatched is_reachable_fn)
            let is_reachable = unsafe { ephemeron_ref.value().is_reachable_fn()(ephemeron, color) };

            if is_reachable {
                still_alive.push(ephemeron);
                continue;
            }
            // copy ptrs for aliasing safety
            let drop_fn = ephemeron_ref.value().drop_fn();

            unsafe { drop_fn(ephemeron) };
            self.allocator
                .borrow_mut()
                .free_slot(ephemeron.cast::<u8>());
        }
        self.ephemeron_queue.borrow_mut().extend(still_alive);

        for node in droppables {
            // copy ptrs for aliasing safety
            let (is_rooted, drop_fn) = {
                let r = unsafe { node.as_ref() };
                (r.value().is_rooted(), r.value().drop_fn())
            };
            // Check one last time if the values are alive in case they were deemed
            // alive while checking the ephemerons.
            if is_rooted {
                still_alive_roots.push(node);
                continue;
            }
            // INVARIANT: free_slot must be called after drop_fn returns and
            // while is_collecting is still true. Violating this would leave the
            // bitmap stale for an allocation that may fire from inside drop_fn.
            debug_assert!(
                self.is_collecting.get(),
                "free_slot called outside a collection — ordering invariant violated"
            );
            unsafe { drop_fn(node) };
            // reclaim the arena slot, clear the bitmap bit and add to free list
            self.allocator.borrow_mut().free_slot(node.cast::<u8>());
        }
        self.root_queue.borrow_mut().extend(still_alive_roots);
    }
}

// Allocator supertrait implementation
//
// allows collections like `Vec<T, &MarkSweepGarbageCollector>` to use
// the GC bump arena as their backing store
//
// rules:
// - `allocate`: returns valid, aligned pointers from the bump arena
// - `deallocate`: decrements active allocations, reclaiming the arena when empty
// - `grow` / `shrink`: allocates new memory and copies the data. the old memory
//    is wasted until the entire arena page is freed, use `Vec::with_capacity`
//    when possible to avoid this waste
//
// SAFETY:
// any raw byte allocation using this impl MUST be stored inside a GC traced
// object. Raw allocations are invisible to the mark phase, so if the owner
// becomes unreachable without the GC knowing, the memory leaks
#[cfg(feature = "gc_allocator")]
unsafe impl allocator_api2::alloc::Allocator for MarkSweepGarbageCollector {
    fn allocate(
        &self,
        layout: allocator_api2::alloc::Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        if layout.size() == 0 {
            // SAFETY: any valid layout has align >= 1.
            let dangling = unsafe { NonNull::new_unchecked(layout.align() as *mut u8) };
            return Ok(NonNull::slice_from_raw_parts(dangling, 0));
        }

        // run any deferred collection before allocating
        if self.collect_needed.get() && !self.is_collecting.get() {
            self.collect_needed.set(false);
            self.collect();
        }

        // raw byte allocations skip ensure_capacity
        // and go straight to try_alloc_bytes
        self.allocator
            .borrow_mut()
            .try_alloc_bytes(layout)
            .map_err(|_| allocator_api2::alloc::AllocError)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, _layout: allocator_api2::alloc::Layout) {
        // decrements active_raw_allocs for the arena containing ptr
        // allowing drop_dead_arenas to reclaim the page when it reaches zero
        self.allocator.borrow_mut().dealloc_bytes(ptr);
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: allocator_api2::alloc::Layout,
        new_layout: allocator_api2::alloc::Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        debug_assert!(
            new_layout.size() >= old_layout.size(),
            "grow called with smaller new_layout"
        );

        // SAFETY:
        // `allocate` may trigger a deferred GC collection, but that is safe here
        // because `collect()` only sweeps GC-traced objects.  `ptr` is a raw
        // arena allocation — invisible to the mark phase — so the sweep will
        // never free it.  Callers MUST NOT pass a GC-managed pointer here.
        debug_assert!(
            !self.is_collecting.get(),
            "grow called from inside a collection; raw pointer may be dangling"
        );
        let new_block = self.allocate(new_layout)?;

        if old_layout.size() > 0 {
            // SAFETY:
            // `ptr` is valid for `old_layout.size()` (guaranteed by caller),
            // `new_block` is fresh and non-overlapping, and the allocator contract
            // guarantees the new alignment is suitable for the old data
            unsafe {
                core::ptr::copy_nonoverlapping(
                    ptr.as_ptr(),
                    new_block.as_ptr() as *mut u8,
                    old_layout.size(),
                );
            }
            unsafe { self.deallocate(ptr, old_layout) };
        }
        Ok(new_block)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: allocator_api2::alloc::Layout,
        new_layout: allocator_api2::alloc::Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2::alloc::AllocError> {
        debug_assert!(
            new_layout.size() <= old_layout.size(),
            "shrink called with larger new_layout"
        );

        if new_layout.size() == 0 {
            // SAFETY: any valid layout has align >= 1
            let dangling = unsafe { NonNull::new_unchecked(new_layout.align() as *mut u8) };
            // Free the old block before returning the ZST dangling pointer.
            unsafe { self.deallocate(ptr, old_layout) };
            return Ok(NonNull::slice_from_raw_parts(dangling, 0));
        }

        //if this is the last allocation in its arena,
        // we can just wind back the bump pointer for a zero-copy O(1) shrink
        let shrunk_in_place = self
            .allocator
            .borrow_mut()
            .shrink_bytes_in_place(ptr, old_layout, new_layout);
        if shrunk_in_place {
            return Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()));
        }

        let new_block = self.allocate(new_layout)?;

        // SAFETY:
        // `ptr` is valid for `old_layout.size()` (caller guarantee)
        // we copy `new_layout.size()` bytes (<= old size) into the fresh
        // block, and the new alignment is suitable for the old data
        unsafe {
            core::ptr::copy_nonoverlapping(
                ptr.as_ptr(),
                new_block.as_ptr() as *mut u8,
                new_layout.size(),
            );
        }
        unsafe { self.deallocate(ptr, old_layout) };
        Ok(new_block)
    }
}

#[cfg(feature = "gc_allocator")]
impl crate::collectors::collector::Collector for MarkSweepGarbageCollector {
    fn collect(&self) {
        MarkSweepGarbageCollector::collect(self);
    }

    fn gc_color(&self) -> TraceColor {
        self.trace_color.get()
    }

    // Allocates a standard GC node for `value`, wrapping it in a `GcBox`
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_gc_node<T: Trace + 'static>(
        &self,
        value: T,
    ) -> Result<ArenaPointer<'static, GcBox<T>>, allocator_api2::alloc::AllocError> {
        if self.collect_needed.get() && !self.is_collecting.get() {
            self.collect_needed.set(false);
            self.collect();
        }

        let gc_box = GcBox::new_in(value, self.trace_color.get());

        // try_alloc creates a new arena page on OOM
        let mut alloc = self.allocator.borrow_mut();
        let arena_ptr = alloc
            .try_alloc(gc_box)
            .map_err(|_| allocator_api2::alloc::AllocError)?;
        let needs_collect = !alloc.is_below_threshold();
        drop(alloc);

        // flag for a deferred collection if the heap crossed its threshold
        if needs_collect {
            self.collect_needed.set(true);
        }

        let erased: NonNull<ArenaHeapItem<GcBox<NonTraceable>>> = arena_ptr.as_ptr().cast();
        if self.is_collecting.get() {
            self.pending_root_queue.borrow_mut().push(erased);
        } else {
            self.root_queue.borrow_mut().push(erased);
        }

        Ok(arena_ptr)
    }

    // Allocates an ephemeron node for a (key, value) pair
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_ephemeron_node<K: Trace + 'static, V: Trace + 'static>(
        &self,
        key: &crate::collectors::mark_sweep::pointers::Gc<K>,
        value: V,
    ) -> Result<ArenaPointer<'static, Ephemeron<K, V>>, allocator_api2::alloc::AllocError> {
        if self.collect_needed.get() && !self.is_collecting.get() {
            self.collect_needed.set(false);
            self.collect();
        }

        let ephemeron = Ephemeron::new(key, value, self.trace_color.get());

        let mut alloc = self.allocator.borrow_mut();
        let inner_ptr = alloc
            .try_alloc(ephemeron)
            .map_err(|_| allocator_api2::alloc::AllocError)?;
        let needs_collect = !alloc.is_below_threshold();
        drop(alloc);

        if needs_collect {
            self.collect_needed.set(true);
        }

        let eph_ptr = inner_ptr
            .as_ptr()
            .cast::<ArenaHeapItem<Ephemeron<NonTraceable, NonTraceable>>>();

        if self.is_collecting.get() {
            self.pending_ephemeron_queue.borrow_mut().push(eph_ptr);
        } else {
            self.ephemeron_queue.borrow_mut().push(eph_ptr);
        }

        Ok(inner_ptr)
    }
}

#[cfg(not(feature = "gc_allocator"))]
impl crate::collectors::collector::Collector for MarkSweepGarbageCollector {
    fn collect(&self) {
        MarkSweepGarbageCollector::collect(self);
    }

    fn gc_color(&self) -> TraceColor {
        self.trace_color.get()
    }

    // Allocates a standard GC node for `value`, wrapping it in a `GcBox`
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_gc_node<T: Trace + 'static>(
        &self,
        value: T,
    ) -> Result<ArenaPointer<'static, GcBox<T>>, crate::alloc::arena3::ArenaAllocError> {
        if self.collect_needed.get() && !self.is_collecting.get() {
            self.collect_needed.set(false);
            self.collect();
        }

        let gc_box = GcBox::new_in(value, self.trace_color.get());

        // try_alloc creates a new arena page on OOM — no pre-creation needed.
        let mut alloc = self.allocator.borrow_mut();
        let arena_ptr = alloc.try_alloc(gc_box)?;
        let needs_collect = !alloc.is_below_threshold();
        drop(alloc);

        // flag for a deferred collection if the heap crossed its threshold
        if needs_collect {
            self.collect_needed.set(true);
        }

        let erased: NonNull<ArenaHeapItem<GcBox<NonTraceable>>> = arena_ptr.as_ptr().cast();
        if self.is_collecting.get() {
            self.pending_root_queue.borrow_mut().push(erased);
        } else {
            self.root_queue.borrow_mut().push(erased);
        }

        Ok(arena_ptr)
    }

    // Allocates an ephemeron node for a (key, value) pair
    //
    // SAFETY:
    // the `'static` pointer is only valid while the collector is alive, do not leak it
    fn alloc_ephemeron_node<K: Trace + 'static, V: Trace + 'static>(
        &self,
        key: &crate::collectors::mark_sweep::pointers::Gc<K>,
        value: V,
    ) -> Result<ArenaPointer<'static, Ephemeron<K, V>>, crate::alloc::arena3::ArenaAllocError> {
        if self.collect_needed.get() && !self.is_collecting.get() {
            self.collect_needed.set(false);
            self.collect();
        }

        let ephemeron = Ephemeron::new(key, value, self.trace_color.get());

        // try_alloc creates a new arena page on OOM
        let mut alloc = self.allocator.borrow_mut();
        let inner_ptr = alloc.try_alloc(ephemeron)?;
        let needs_collect = !alloc.is_below_threshold();
        drop(alloc);

        if needs_collect {
            self.collect_needed.set(true);
        }

        let eph_ptr = inner_ptr
            .as_ptr()
            .cast::<ArenaHeapItem<Ephemeron<NonTraceable, NonTraceable>>>();

        if self.is_collecting.get() {
            self.pending_ephemeron_queue.borrow_mut().push(eph_ptr);
        } else {
            self.ephemeron_queue.borrow_mut().push(eph_ptr);
        }

        Ok(inner_ptr)
    }
}
