//! arena allocator managing multiple backing arenas
//!
//! typed GC objects use arenas matching their size class, sharing pools for reuse.
//! raw byte allocations live on separate pages

use core::{cell::Cell, ptr::NonNull};
use rust_alloc::alloc::{Layout, LayoutError};
use rust_alloc::vec::Vec;

mod alloc;

use alloc::Arena;
pub use alloc::{ArenaHeapItem, ArenaPointer, ErasedArenaPointer};

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

const SIZE_CLASSES: &[usize] = &[16, 24, 32, 48, 64, 96, 128, 192, 256, 512, 1024, 2048];

fn size_class_index_for(size: usize) -> usize {
    let idx = SIZE_CLASSES.iter().copied().position(|sc| sc >= size);
    debug_assert!(
        idx.is_some(),
        "object size {size}B exceeds the largest size class ({}B); \
         consider adding a larger class",
        SIZE_CLASSES.last().unwrap()
    );
    idx.unwrap_or(SIZE_CLASSES.len() - 1)
}

const DEFAULT_ARENA_SIZE: usize = 4096;
const DEFAULT_HEAP_THRESHOLD: usize = 2_097_152;

#[derive(Debug)]
pub struct ArenaAllocator<'alloc> {
    pub(crate) heap_threshold: usize,
    pub(crate) arena_size: usize,
    pub(crate) current_heap_size: usize,
    // all typed GC arenas
    pub(crate) typed_arenas: Vec<Arena>,
    // arenas dedicated to raw byte allocations
    pub(crate) raw_arenas: Vec<Arena>,
    pub(crate) free_cache: Cell<usize>,
    pub(crate) alloc_cache: [Cell<usize>; 12],
    _marker: core::marker::PhantomData<&'alloc ()>,
}

impl<'alloc> Default for ArenaAllocator<'alloc> {
    fn default() -> Self {
        Self {
            heap_threshold: DEFAULT_HEAP_THRESHOLD,
            arena_size: DEFAULT_ARENA_SIZE,
            current_heap_size: 0,
            typed_arenas: Vec::new(),
            raw_arenas: Vec::new(),
            free_cache: Cell::new(usize::MAX),
            alloc_cache: [
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
                Cell::new(usize::MAX),
            ],
            _marker: core::marker::PhantomData,
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

    // total live arena count
    pub fn arenas_len(&self) -> usize {
        self.typed_arenas.len() + self.raw_arenas.len()
    }

    // exact heap size in bytes
    fn heap_size(&self) -> usize {
        self.current_heap_size
    }

    pub fn is_below_threshold(&self) -> bool {
        // keep 25% headroom so collection fires before the last page fills
        let margin = self.heap_threshold / 4;
        self.heap_size() <= self.heap_threshold.saturating_sub(margin)
    }

    pub fn increase_threshold(&mut self) {
        self.heap_threshold += self.arena_size * 4;
    }
}

impl<'alloc> ArenaAllocator<'alloc> {
    pub fn try_alloc<T>(&mut self, value: T) -> Result<ArenaPointer<'alloc, T>, ArenaAllocError> {
        let needed = core::mem::size_of::<ArenaHeapItem<T>>().max(8);
        let sc_idx = size_class_index_for(needed);
        let slot_size = SIZE_CLASSES.get(sc_idx).copied().unwrap_or(needed);

        let cached_idx = self.alloc_cache[sc_idx].get();
        if cached_idx < self.typed_arenas.len() {
            let arena = &self.typed_arenas[cached_idx];
            if arena.slot_size == slot_size {
                if let Some(slot_ptr) = arena.alloc_slot() {
                    // SAFETY: slot_ptr was successfully allocated for this size class
                    return unsafe {
                        let dst = slot_ptr.as_ptr() as *mut ArenaHeapItem<T>;
                        dst.write(ArenaHeapItem(value));
                        Ok(ArenaPointer::from_raw(NonNull::new_unchecked(dst)))
                    };
                }
            }
        }

        // try existing arenas with matching slot_size first
        for (i, arena) in self.typed_arenas.iter().enumerate().rev() {
            if arena.slot_size == slot_size {
                if let Some(slot_ptr) = arena.alloc_slot() {
                    self.alloc_cache[sc_idx].set(i);
                    // SAFETY: slot_ptr was successfully allocated for this size class
                    return unsafe {
                        let dst = slot_ptr.as_ptr() as *mut ArenaHeapItem<T>;
                        dst.write(ArenaHeapItem(value));
                        Ok(ArenaPointer::from_raw(NonNull::new_unchecked(dst)))
                    };
                }
            }
        }

        // need a new arena for this size class
        let total = self.arena_size.max(slot_size * 4);
        let new_arena = Arena::try_init(slot_size, total, 16)?;
        self.current_heap_size += new_arena.layout.size();
        let slot_ptr = new_arena.alloc_slot().ok_or(ArenaAllocError::OutOfMemory)?;
        let insert_idx = self.typed_arenas.len();
        self.typed_arenas.push(new_arena);
        self.alloc_cache[sc_idx].set(insert_idx);

        // SAFETY: slot_ptr was successfully allocated for this size class
        unsafe {
            let dst = slot_ptr.as_ptr() as *mut ArenaHeapItem<T>;
            dst.write(ArenaHeapItem(value));
            Ok(ArenaPointer::from_raw(NonNull::new_unchecked(dst)))
        }
    }

    pub fn free_slot(&mut self, ptr: NonNull<u8>) {
        let cached = self.free_cache.get();
        if cached < self.typed_arenas.len() {
            let arena = &self.typed_arenas[cached];
            if arena.owns(ptr) {
                arena.free_slot(ptr);
                return;
            }
        }

        for (i, arena) in self.typed_arenas.iter().enumerate().rev() {
            if arena.owns(ptr) {
                arena.free_slot(ptr);
                self.free_cache.set(i);
                return;
            }
        }
        debug_assert!(
            false,
            "free_slot called with pointer {ptr:p} not owned by any typed arena; \
             possible double-free or pointer from a raw arena"
        );
    }

    // bump allocate raw bytes
    pub fn try_alloc_bytes(&mut self, layout: Layout) -> Result<NonNull<[u8]>, ArenaAllocError> {
        // try the most recent raw arena first
        if let Some(arena) = self.raw_arenas.last() {
            if let Ok(ptr) = arena.try_alloc_bytes(layout) {
                return Ok(ptr);
            }
        }
        // allocate a new raw page with margin for padding
        let margin = 64; // ~4 bitmap words + alignment gaps
        let total = self.arena_size.max(layout.size() + layout.align() + margin);
        let max_align = layout.align().max(16);
        let raw_arena = Arena::try_init(8, total, max_align)?;
        self.current_heap_size += raw_arena.layout.size();
        let ptr = raw_arena
            .try_alloc_bytes(layout)
            .map_err(|_| ArenaAllocError::OutOfMemory)?;
        self.raw_arenas.push(raw_arena);
        Ok(ptr)
    }

    // decrement raw allocation counter for the arena owning ptr
    pub fn dealloc_bytes(&mut self, ptr: NonNull<u8>) {
        let target = ptr.as_ptr() as usize;
        for arena in self.raw_arenas.iter().rev() {
            let start = arena.buffer.as_ptr() as usize;
            let end = start + arena.layout.size();
            if target >= start && target < end {
                arena.dealloc_bytes();
                return;
            }
        }
    }

    // try to shrink a raw allocation in place
    pub fn shrink_bytes_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> bool {
        let target = ptr.as_ptr() as usize;
        for arena in self.raw_arenas.iter().rev() {
            let start = arena.buffer.as_ptr() as usize;
            let end = start + arena.layout.size();

            if target >= start && target < end {
                let current_bump = arena.bump.get();
                let allocation_end = target - start + old_layout.size();

                if allocation_end == current_bump {
                    let new_allocation_end = target - start + new_layout.size();
                    arena.bump.set(new_allocation_end);
                    return true;
                }

                return false;
            }
        }

        false
    }

    // drop empty typed and raw arenas
    pub fn drop_dead_arenas(&mut self) {
        self.typed_arenas.retain(|a| {
            if a.run_drop_check() {
                self.current_heap_size = self.current_heap_size.saturating_sub(a.layout.size());
                false
            } else {
                true
            }
        });
        self.raw_arenas.retain(|a| {
            if a.run_drop_check() {
                self.current_heap_size = self.current_heap_size.saturating_sub(a.layout.size());
                false
            } else {
                true
            }
        });
        self.free_cache.set(usize::MAX);
        for cache in &self.alloc_cache {
            cache.set(usize::MAX);
        }
    }

    // mark the slot at `ptr` as occupied
    pub fn mark_slot(&self, ptr: NonNull<u8>) {
        for arena in self.typed_arenas.iter().chain(self.raw_arenas.iter()) {
            if arena.owns(ptr) {
                arena.mark_slot(ptr);
                return;
            }
        }
    }
}
