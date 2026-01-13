//! An Arena allocator that manages multiple backing arenas

use rust_alloc::alloc::LayoutError;
use rust_alloc::collections::LinkedList;

mod alloc;

use alloc::{Arena, ArenaPtr};

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

// NOTE: Vec may actually be better here over link list.

// Set the default page 4kb
//
// We can change this as needed later
const DEFAULT_ARENA_SIZE: usize = 4096;

pub struct ArenaAllocator<'alloc> {
    arena_size: usize,
    arenas: LinkedList<Arena<'alloc>>,
}

impl<'alloc> Default for ArenaAllocator<'alloc> {
    fn default() -> Self {
        Self {
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

    pub fn arenas_len(&self) -> usize {
        self.arenas.len()
    }
}

impl<'alloc> ArenaAllocator<'alloc> {
    pub fn try_alloc<T>(&mut self, value: T) -> Result<ArenaPtr<'alloc, T>, ArenaAllocError> {
        let active = match self.get_active_arena_mut() {
            Some(arena) => arena,
            None => {
                // TODO: don't hard code alignment
                //
                // TODO: also, we need a min-alignment
                self.initialize_new_arena()?;
                self.get_active_arena_mut()
                    .expect("must exist, we just set it")
            }
        };

        match active.get_allocation_data(&value) {
            // SAFETY: TODO
            Ok(data) => unsafe { Ok(active.alloc_unchecked::<T>(value, data)) },
            Err(ArenaAllocError::OutOfMemory) => {
                self.initialize_new_arena()?;
                let new_active = self.get_active_arena_mut().expect("must exist, ");
                new_active.try_alloc(value)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn initialize_new_arena(&mut self) -> Result<(), ArenaAllocError> {
        let new_arena = Arena::try_init(self.arena_size, 16)?;
        self.arenas.push_front(new_arena);
        Ok(())
    }

    pub fn get_active_arena_mut(&mut self) -> Option<&mut Arena<'alloc>> {
        self.arenas.front_mut()
    }

    pub fn drop_dead_arenas(&mut self) {
        for dead_arenas in self.arenas.extract_if(|a| a.run_drop_check()) {
            drop(dead_arenas)
        }
    }
}
