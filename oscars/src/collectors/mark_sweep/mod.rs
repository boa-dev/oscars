//! An implementation of a mark sweep collector
//!
//! This was initially a copy of `boa_gc` with alterations to make the collector
//! `no_std`

use core::ptr::NonNull;

use crate::{
    alloc::arena2::{ArenaAllocator, ArenaHeapItem, ArenaPointer},
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

pub use trace::{Finalize, Trace, TraceColor};

pub use pointers::{Gc, WeakGc, WeakMap};

type GcErasedPointer = NonNull<ArenaHeapItem<GcBox<NonTraceable>>>;
type ErasedEphemeron = NonNull<ArenaHeapItem<Ephemeron<NonTraceable, NonTraceable>>>;

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
    pub(crate) allocator: ArenaAllocator<'static>, // TODO: Cell or refcell
    root_queue: Vec<GcErasedPointer>,
    ephemeron_queue: Vec<ErasedEphemeron>,
    state: CollectionState,
    // weak maps allocated via WeakMap::new, owned here so their lifetime is
    // tied to the collector
    pub(crate) weak_maps: Vec<rust_alloc::boxed::Box<dyn ErasedWeakMap>>,
}

impl core::fmt::Debug for MarkSweepGarbageCollector {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MarkSweepGarbageCollector")
            .field("allocator", &self.allocator)
            .field("root_queue", &self.root_queue)
            .field("ephemeron_queue", &self.ephemeron_queue)
            .field("state", &self.state)
            .field("weak_maps_count", &self.weak_maps.len())
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct CollectionState {
    color: TraceColor,
}

impl MarkSweepGarbageCollector {
    pub fn with_heap_threshold(mut self, heap_threshold: usize) -> Self {
        self.allocator = self.allocator.with_heap_threshold(heap_threshold);
        self
    }

    pub fn with_arena_size(mut self, arena_size: usize) -> Self {
        self.allocator = self.allocator.with_arena_size(arena_size);
        self
    }

    #[cfg(test)]
    pub fn root_queue_len(&self) -> usize {
        self.root_queue.len()
    }

    #[cfg(test)]
    pub fn ephemeron_queue_len(&self) -> usize {
        self.ephemeron_queue.len()
    }
}

// ==== Allocation methods ====

impl MarkSweepGarbageCollector {
    pub fn check_allocation<T>(&mut self, value: &T) {
        let allocation_data = self.allocator.get_allocation_data(value);
        match allocation_data {
            Ok(Some(_)) => {}
            _ if self.allocator.is_below_threshold() => self
                .allocator
                .initialize_new_arena()
                .expect("Unable to request region from system"),
            _ => {
                self.collect();
                // If the collection did not free any memory, then bump the
                // threshold, and initialize a new arena
                if !self.allocator.is_below_threshold() {
                    self.allocator.increase_threshold();
                    self.allocator
                        .initialize_new_arena()
                        .expect("Unable to request region from system")
                }
            }
        };
    }

    pub fn alloc_with_collection<T: Trace>(
        &mut self,
        gc_box: GcBox<T>,
    ) -> ArenaPointer<'static, GcBox<T>> {
        // This method checks the allocation and triggers a collection if needed.
        self.check_allocation(&gc_box);

        // We need to update the mark of the gc_box as it could be desynced from
        // the collection state
        gc_box.set_unmarked(&self.state);

        // Allocate it onto the heap.
        let arena_ptr = self
            .allocator
            .try_alloc(gc_box)
            .expect("Failed to allocate memory");

        // TODO (addressed?): This is problematic and may cost performance.
        //
        // We are allocating the Box randomly on the heap and not into an arena.
        //
        // There may be some value here to use Bumpalo as an optimization.
        //
        // Another option would be to create a Vec type backed by a single arena. The
        // reason for this is because our NeoGcBox should be singular inside, so we
        // have some valid options here that could be useful for this.
        //
        // Funny enough, this is probably a great use for mempool... would two allocators
        // be too much?
        //
        // Although, the long term solution would be to move more and more functionality
        // into the allocator pointer, but that could saved for another day.
        //

        // Create an erased pointer to the heap object for the collector queue
        // SAFETY: The erased pointer is used to determine whether the value is dropped.
        let erased: NonNull<ArenaHeapItem<GcBox<NonTraceable>>> = arena_ptr.as_ptr().cast();
        self.root_queue.push(erased);

        arena_ptr
    }

    pub fn alloc_epemeron_with_collection<K: Trace, V: Trace>(
        &mut self,
        ephemeron: Ephemeron<K, V>,
    ) -> ArenaPointer<'static, Ephemeron<K, V>> {
        // Checks if there is room for an allocation and triggers a collection if not
        // enough space on the heap
        self.check_allocation(&ephemeron);
        // Updates the ephemron for the new allocation state.
        ephemeron.set_unmarked(&self.state);
        let inner_ptr = self
            .allocator
            .try_alloc(ephemeron)
            .expect("failed to allocate");

        // Push to root stack
        let eph_ptr = inner_ptr
            .as_ptr()
            .cast::<ArenaHeapItem<Ephemeron<NonTraceable, NonTraceable>>>();
        self.ephemeron_queue.push(eph_ptr);

        inner_ptr
    }
}

// ==== Collection methods ====

impl MarkSweepGarbageCollector {
    pub fn collect(&mut self) {
        self.run_mark_phase();
        self.run_sweep_phase();
        // We've run a collection, so we switch the color.
        self.state.color = self.state.color.flip();
        // NOTE: It would actually be interesting to reuse the arenas that are dead rather
        // than drop the page and reallocate when a new page is needed ... TBD

        // prune dead entries from each collector owned weak map before freeing
        // memory so we can still inspect the dropped flag on ephemerons
        for map in &mut self.weak_maps {
            map.prune_dead_entries();
        }

        self.allocator.drop_dead_arenas();
    }

    pub fn run_mark_phase(&mut self) {
        // Run marks through the roots
        for heap_item in &self.root_queue {
            let heap_item_ref = unsafe { heap_item.as_ref() };
            if heap_item_ref.value().is_rooted() {
                unsafe {
                    heap_item_ref.value().trace_fn()(*heap_item, self.state.color);
                }
            }
        }

        for ephemeron_heap_item in &self.ephemeron_queue {
            let ephemeron_ref = unsafe { ephemeron_heap_item.as_ref() };
            // use the vtable to check if the key is still reachable
            let is_reachable = unsafe {
                ephemeron_ref.value().is_reachable_fn()(*ephemeron_heap_item, self.state.color)
            };
            if is_reachable {
                unsafe { ephemeron_ref.value().trace_fn()(*ephemeron_heap_item, self.state.color) }
            }
        }

        // At this point, all objects should be marked.
    }

    pub fn run_sweep_phase(&mut self) {
        // NOTE: it is important here to only extract after attemmpting to finalize. This is
        // so that our queues ideally maintain the insertion order for so that they are cache
        // friendly.
        let droppables = self.root_queue.extract_if(.., |node| {
            let heap_item_ref = unsafe { node.as_ref() };
            let gc_box = heap_item_ref.value();
            // Check if the value is not reachable, i.e. dead.
            if !gc_box.is_reachable(self.state.color) {
                // Finalize the dead item
                gc_box.finalize();
                // Recheck if the value is now rooted again after finalization.
                if gc_box.is_rooted() {
                    unsafe { gc_box.trace_fn()(*node, self.state.color) };
                }
            }
            // Extract if the value is still no longer reachable.
            !heap_item_ref.value().is_reachable(self.state.color)
        });

        let ephemerons = self.ephemeron_queue.extract_if(.., |node| {
            let heap_item_ref = unsafe { node.as_ref() };
            let ephemeron = heap_item_ref.value();
            let color = self.state.color;

            // check if the key is reachable via the vtable
            let is_reachable = unsafe { ephemeron.is_reachable_fn()(*node, color) };

            if !is_reachable {
                unsafe { ephemeron.finalize_fn()(*node) };
                // check if key was revived by finalizer
                let revived = unsafe { ephemeron.is_reachable_fn()(*node, color) };
                if revived {
                    unsafe { ephemeron.trace_fn()(*node, color) };
                }
            }

            // sweep if key is still dead after finalization
            !unsafe { ephemeron.is_reachable_fn()(*node, color) }
        });

        let mut still_alive = Vec::default();
        for node in droppables {
            // copy ptrs for aliasing safety
            let (is_rooted, drop_fn) = {
                let r = unsafe { node.as_ref() };
                (r.value().is_rooted(), r.value().drop_fn())
            };
            // Check one last time if the values are alive in case they were deemed
            // alive while checking the ephemerons.
            if is_rooted {
                still_alive.push(node);
                continue;
            }
            unsafe { drop_fn(node) }
        }
        self.root_queue.extend(still_alive);

        let mut still_alive = Vec::default();
        for ephemeron in ephemerons {
            // copy ptrs for aliasing safety
            let (is_reachable_fn, drop_fn) = {
                let r = unsafe { ephemeron.as_ref() };
                (r.value().is_reachable_fn(), r.value().drop_fn())
            };
            if unsafe { is_reachable_fn(ephemeron, self.state.color) } {
                still_alive.push(ephemeron);
                continue;
            }
            unsafe { drop_fn(ephemeron) }
        }
        self.ephemeron_queue.extend(still_alive);
    }
}
