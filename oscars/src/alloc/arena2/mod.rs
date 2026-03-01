//! An Arena allocator that manages multiple backing arenas

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
//
// But this may all be something to work on in arena3

// NOTE: Vec may actually be better here over link list.

// Set the default page 4kb
//
// We can change this as needed later
const DEFAULT_ARENA_SIZE: usize = 4096;

/// Default upper limit of 2MB (2 ^ 21)
const DEFAULT_HEAP_THRESHOLD: usize = 2_097_152;

#[derive(Debug)]
pub struct ArenaAllocator<'alloc> {
    heap_threshold: usize,
    arena_size: usize,
    arenas: LinkedList<Arena<'alloc>>,
}

impl<'alloc> Default for ArenaAllocator<'alloc> {
    fn default() -> Self {
        Self {
            heap_threshold: DEFAULT_HEAP_THRESHOLD,
            arena_size: DEFAULT_ARENA_SIZE,
            arenas: LinkedList::default(),
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

    pub fn arenas_len(&self) -> usize {
        self.arenas.len()
    }

    pub fn heap_size(&self) -> usize {
        self.arenas_len() * self.arena_size
    }

    pub fn is_below_threshold(&self) -> bool {
        self.heap_size() <= self.heap_threshold - self.arena_size
    }

    pub fn increase_threshold(&mut self) {
        self.heap_threshold += self.arena_size * 4
    }
}

impl<'alloc> ArenaAllocator<'alloc> {
    pub fn try_alloc<T>(&mut self, value: T) -> Result<ArenaPointer<'alloc, T>, ArenaAllocError> {
        let active = match self.get_active_arena() {
            Some(arena) => arena,
            None => {
                // TODO: don't hard code alignment
                //
                // TODO: also, we need a min-alignment
                self.initialize_new_arena()?;
                self.get_active_arena().expect("must exist, we just set it")
            }
        };

        match active.get_allocation_data(&value) {
            // SAFETY: TODO
            Ok(data) => unsafe { Ok(active.alloc_unchecked::<T>(value, data)) },
            Err(ArenaAllocError::OutOfMemory) => {
                self.initialize_new_arena()?;
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

    pub fn initialize_new_arena(&mut self) -> Result<(), ArenaAllocError> {
        let new_arena = Arena::try_init(self.arena_size, 16)?;
        self.arenas.push_front(new_arena);
        Ok(())
    }

    pub fn get_active_arena(&self) -> Option<&Arena<'alloc>> {
        self.arenas.front()
    }

    pub fn drop_dead_arenas(&mut self) {
        for dead_arenas in self.arenas.extract_if(|a| a.run_drop_check()) {
            drop(dead_arenas)
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
