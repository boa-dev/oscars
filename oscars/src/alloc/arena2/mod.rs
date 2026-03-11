//! An Arena allocator that manages multiple backing arenas

use core::mem;

use rust_alloc::alloc::LayoutError;
use rust_alloc::collections::LinkedList;

mod alloc;

use alloc::Arena;
pub use alloc::{
    ArenaAllocationData, ArenaHeapItem, ArenaPointer, ErasedArenaPointer, ErasedHeapItem,
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum ArenaAllocError {
    LayoutError(LayoutError),
    OutOfMemory,
    AlignmentNotPossible,
}

impl From<LayoutError> for ArenaAllocError {
    fn from(value: LayoutError) -> Self {
        Self::LayoutError(value)
    }
}

// TODO: reconcile logic around lifetimes in arena allocator.
//
// The current approach is notably incorrect when it comes to lifetimes, we would
// be completely unable to reasonably expose this allocator beyond our own GC
// without doing so. Currently, this is "safe" because we know that the heap box
// will be GC'd, Rust nor the compiler knows that, and so when lifetimes are
// properly applied, this won't compile.
//
// This may also point to a different problem which is that the arena's as they
// currently exist do not have a lifetime, their lifetime is derived from the
// ArenaAllocator.

// NOTE: Vec may actually be better here over link list.

// Set the default page 4kb
//
// We can change this as needed later
const DEFAULT_ARENA_SIZE: usize = 4096;

/// Default upper limit of 2MB (2 ^ 21)
const DEFAULT_HEAP_THRESHOLD: usize = 2_097_152;

/// Minimum guaranteed alignment for every arena buffer.
const DEFAULT_MIN_ALIGNMENT: usize = 8;

/// Maximum number of idle arenas held (4 idle pages x 4KB = 16KB of OS memory pressure buffered)
const MAX_RECYCLED_ARENAS: usize = 4;

#[derive(Debug)]
pub struct ArenaAllocator<'alloc> {
    heap_threshold: usize,
    arena_size: usize,
    min_alignment: usize,
    arenas: LinkedList<Arena<'alloc>>,
    // empty arenas kept alive to avoid OS reallocation on the next cycle
    recycled_arenas: [Option<Arena<'alloc>>; MAX_RECYCLED_ARENAS],
    // number of idle arenas currently held
    recycled_count: usize,
}

impl<'alloc> Default for ArenaAllocator<'alloc> {
    fn default() -> Self {
        Self {
            heap_threshold: DEFAULT_HEAP_THRESHOLD,
            arena_size: DEFAULT_ARENA_SIZE,
            min_alignment: DEFAULT_MIN_ALIGNMENT,
            arenas: LinkedList::default(),
            recycled_arenas: core::array::from_fn(|_| None),
            recycled_count: 0,
        }
    }
}

impl<'alloc> ArenaAllocator<'alloc> {
    pub fn with_arena_size(mut self, arena_size: usize) -> Self {
        self.arena_size = arena_size;
        self
    }
    pub fn with_heap_threshold(mut self, heap_threshold: usize) -> Self {
        self.heap_threshold = heap_threshold;
        self
    }
    /// Override the baseline alignment for every new arena buffer.
    pub fn with_min_alignment(mut self, min_alignment: usize) -> Self {
        self.min_alignment = min_alignment;
        self
    }

    pub fn arenas_len(&self) -> usize {
        self.arenas.len()
    }

    pub fn heap_size(&self) -> usize {
        // recycled arenas hold no live objects, exclude them from GC pressure
        self.arenas_len() * self.arena_size
    }

    pub fn is_below_threshold(&self) -> bool {
        // saturating_sub avoids underflow when heap_threshold < arena_size
        self.heap_size() <= self.heap_threshold.saturating_sub(self.arena_size)
    }

    pub fn increase_threshold(&mut self) {
        self.heap_threshold += self.arena_size * 4
    }
}

impl<'alloc> ArenaAllocator<'alloc> {
    pub fn try_alloc<T>(&mut self, value: T) -> Result<ArenaPointer<'alloc, T>, ArenaAllocError> {
        // Determine the minimum alignment this type requires.
        let required_alignment = mem::align_of::<alloc::ArenaHeapItem<T>>();

        let active = match self.get_active_arena() {
            Some(arena) => arena,
            None => {
                self.initialize_new_arena(required_alignment)?;
                self.get_active_arena().expect("must exist, we just set it")
            }
        };

        match active.get_allocation_data(&value) {
            // SAFETY: TODO
            Ok(data) => unsafe { Ok(active.alloc_unchecked::<T>(value, data)) },
            // The active arena is either full or was created with an alignment
            // that is too small for this type. Either way, close it and spin up
            // a fresh arena that satisfies the alignment requirement.
            Err(ArenaAllocError::OutOfMemory | ArenaAllocError::AlignmentNotPossible) => {
                active.close();
                self.initialize_new_arena(required_alignment)?;
                let new_active = self.get_active_arena().expect("must exist");
                new_active.try_alloc(value)
            }
            Err(e) => Err(e),
        }
    }

    pub fn get_allocation_data<T>(
        &self,
        value: &T,
    ) -> Result<Option<ArenaAllocationData>, ArenaAllocError> {
        self.arenas
            .front()
            .map(|a| a.get_allocation_data(value))
            .transpose()
    }

    /// Initialize a fresh arena, attempting to reuse a recycled one first.
    pub fn initialize_new_arena(
        &mut self,
        required_alignment: usize,
    ) -> Result<(), ArenaAllocError> {
        let alignment = self.min_alignment.max(required_alignment);

        // Check the recycle list first to avoid an OS allocation.
        if self.recycled_count > 0 {
            self.recycled_count -= 1;
            if let Some(recycled) = self.recycled_arenas[self.recycled_count].take() {
                // arena.reset() was already called when it was parked.
                // Only reuse if its original alignment satisfies the current requirement,
                // otherwise drop it and fall through to a fresh OS allocation.
                if recycled.layout.align() >= alignment {
                    self.arenas.push_front(recycled);
                    return Ok(());
                }
            }
        }

        let new_arena = Arena::try_init(self.arena_size, alignment)?;
        self.arenas.push_front(new_arena);
        Ok(())
    }

    pub fn get_active_arena(&self) -> Option<&Arena<'alloc>> {
        self.arenas.front()
    }

    pub fn drop_dead_arenas(&mut self) {
        for arena in self.arenas.extract_if(|a| a.run_drop_check()) {
            if self.recycled_count < MAX_RECYCLED_ARENAS {
                //reset in place and park in the reserve.
                arena.reset();
                self.recycled_arenas[self.recycled_count] = Some(arena);
                self.recycled_count += 1;
            }
            // else: arena drops here, returning memory to the OS
        }
    }

    // checks dropped items across all arenas
    #[cfg(test)]
    pub fn arena_drop_states(&self) -> rust_alloc::vec::Vec<rust_alloc::vec::Vec<bool>> {
        self.arenas
            .iter()
            .map(|arena| arena.item_drop_states())
            .collect()
    }
}
