use rust_alloc::vec::Vec;

mod alloc;
pub use alloc::{ArenaAllocError, ArenaPointer, BitmapArena};

const DEFAULT_ARENA_SIZE: usize = 4096;

/// Default upper limit of 2MB (2 ^ 21)
const DEFAULT_HEAP_THRESHOLD: usize = 2_097_152;

#[derive(Debug)]
pub struct ArenaAllocator<'alloc> {
    heap_threshold: usize,
    arena_size: usize,
    current_arena_idx: usize,
    arenas: Vec<BitmapArena<'alloc>>,
}

impl<'alloc> Default for ArenaAllocator<'alloc> {
    fn default() -> Self {
        Self {
            heap_threshold: DEFAULT_HEAP_THRESHOLD,
            arena_size: DEFAULT_ARENA_SIZE,
            arenas: Vec::new(),
            current_arena_idx: 0,
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
        let active = match self.get_active_arena_mut() {
            Some(arena) => arena,
            None => {
                self.initialize_new_arena()?;
                self.get_active_arena_mut().expect("must exist, we just set it")
            }
        };

        match active.get_allocation_data::<T>() {
            Ok(data) => unsafe { Ok(active.alloc_unchecked::<T>(value, data)) },
            Err(ArenaAllocError::OutOfMemory) => {
                self.initialize_new_arena()?;
                let new_active = self.get_active_arena_mut().expect("must exist");
                new_active.try_alloc(value)
            }
            Err(e) => Err(e),
        }
    }

    pub fn initialize_new_arena(&mut self) -> Result<(), ArenaAllocError> {
        let new_arena = BitmapArena::new(self.arena_size, 16)?;
        self.arenas.push(new_arena);
        self.current_arena_idx = self.arenas.len() - 1;
        Ok(())
    }

    pub fn get_active_arena_mut(&mut self) -> Option<&mut BitmapArena<'alloc>> {
        self.arenas.last_mut()
    }

    pub fn drop_dead_arenas(&mut self) {}
}
