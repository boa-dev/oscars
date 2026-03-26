//! size-class memory pool. typed GC objects go into per size class slot pools
//! where freed slots are recycled via a free list, raw byte allocations use
//! separate bump pages

use core::{cell::Cell, ptr::NonNull};
use rust_alloc::alloc::{Layout, LayoutError};
use rust_alloc::vec::Vec;

mod alloc;

use alloc::{BumpPage, SlotPool};
pub use alloc::{ErasedPoolPointer, PoolItem, PoolPointer};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum PoolAllocError {
    LayoutError(LayoutError),
    OutOfMemory,
    AlignmentNotPossible,
}

impl From<LayoutError> for PoolAllocError {
    fn from(value: LayoutError) -> Self {
        Self::LayoutError(value)
    }
}

const SIZE_CLASSES: &[usize] = &[16, 24, 32, 48, 64, 96, 128, 192, 256, 512, 1024, 2048];

#[inline(always)]
fn size_class_index_for(size: usize) -> usize {
    // binary search over size classes
    let idx = SIZE_CLASSES.partition_point(|&sc| sc < size);
    debug_assert!(
        idx < SIZE_CLASSES.len(),
        "object size {size}B exceeds the largest size class ({}B); \
         consider adding a larger class",
        SIZE_CLASSES.last().unwrap()
    );
    idx.min(SIZE_CLASSES.len() - 1)
}

const DEFAULT_PAGE_SIZE: usize = 262_144;
const DEFAULT_HEAP_THRESHOLD: usize = 2_097_152;

#[derive(Debug)]
pub struct PoolAllocator<'alloc> {
    pub(crate) heap_threshold: usize,
    pub(crate) page_size: usize,
    pub(crate) current_heap_size: usize,
    // per size-class slot pools
    pub(crate) slot_pools: Vec<SlotPool>,
    // bump pages for raw byte allocs
    pub(crate) bump_pages: Vec<BumpPage>,
    // cached index of the last pool used by free_slot
    pub(crate) free_cache: Cell<usize>,
    // per size class cached index of the last pool used by alloc_slot
    pub(crate) alloc_cache: [Cell<usize>; 12],
    // empty slot pools kept alive to avoid OS reallocation on the next cycle
    pub(crate) recycled_pools: Vec<SlotPool>,
    // maximum number of idle pages held across all size classes
    pub(crate) max_recycled: usize,
    // sorted (slot_base, slot_end, pool_idx) index for O(log n) lookups
    pub(crate) sorted_ranges: Vec<(usize, usize, usize)>,

    _marker: core::marker::PhantomData<&'alloc ()>,
}

impl<'alloc> Default for PoolAllocator<'alloc> {
    fn default() -> Self {
        Self {
            heap_threshold: DEFAULT_HEAP_THRESHOLD,
            page_size: DEFAULT_PAGE_SIZE,
            current_heap_size: 0,
            slot_pools: Vec::new(),
            bump_pages: Vec::new(),
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
            recycled_pools: Vec::new(),
            // keep two empty pages per size class to reduce OS overhead
            max_recycled: SIZE_CLASSES.len() * 2,
            sorted_ranges: Vec::new(),

            _marker: core::marker::PhantomData,
        }
    }
}

impl<'alloc> PoolAllocator<'alloc> {
    pub fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size;
        self
    }
    pub fn with_heap_threshold(mut self, heap_threshold: usize) -> Self {
        self.heap_threshold = heap_threshold;
        self
    }

    /// total live slot pool + bump page count
    pub fn pools_len(&self) -> usize {
        self.slot_pools.len() + self.bump_pages.len()
    }

    /// exact heap size in bytes
    fn heap_size(&self) -> usize {
        self.current_heap_size
    }

    pub fn is_below_threshold(&self) -> bool {
        // keep 25% headroom so collection fires before the last page fills
        let margin = self.heap_threshold / 4;
        self.heap_size() <= self.heap_threshold.saturating_sub(margin)
    }

    pub fn increase_threshold(&mut self) {
        self.heap_threshold += self.page_size * 4;
    }
}

impl<'alloc> PoolAllocator<'alloc> {
    /// rebuild `sorted_ranges` from current `slot_pools`
    ///
    /// needed because removing empty pools changes the indices
    fn rebuild_sorted_ranges(&mut self) {
        self.sorted_ranges.clear();
        for (i, pool) in self.slot_pools.iter().enumerate() {
            let (base, end) = pool.slot_range();
            self.sorted_ranges.push((base, end, i));
        }
        self.sorted_ranges
            .sort_unstable_by_key(|&(base, _, _)| base);
    }

    /// binary search `sorted_ranges` for the pool owning `ptr`
    ///
    /// returns the `slot_pools` index or `None` if it belongs to a bump page
    #[inline]
    fn find_pool_idx(&self, ptr: NonNull<u8>) -> Option<usize> {
        let addr = ptr.as_ptr() as usize;
        // partition_point finds the first entry where slot_base > addr,
        // so the candidate is at index - 1
        let idx = self
            .sorted_ranges
            .partition_point(|&(base, _, _)| base <= addr);
        if idx == 0 {
            return None;
        }
        let &(_, end, pool_idx) = &self.sorted_ranges[idx - 1];
        if addr < end { Some(pool_idx) } else { None }
    }

    #[inline]
    pub fn try_alloc<T>(&mut self, value: T) -> Result<PoolPointer<'alloc, T>, PoolAllocError> {
        let needed = core::mem::size_of::<PoolItem<T>>().max(8);
        let sc_idx = size_class_index_for(needed);
        let slot_size = SIZE_CLASSES.get(sc_idx).copied().unwrap_or(needed);

        let cached_idx = self.alloc_cache[sc_idx].get();
        if cached_idx < self.slot_pools.len() {
            let pool = &self.slot_pools[cached_idx];
            if pool.slot_size == slot_size
                && let Some(slot_ptr) = pool.alloc_slot()
            {
                // SAFETY: slot_ptr was successfully allocated for this size class
                return unsafe {
                    let dst = slot_ptr.as_ptr() as *mut PoolItem<T>;
                    dst.write(PoolItem(value));
                    Ok(PoolPointer::from_raw(NonNull::new_unchecked(dst)))
                };
            }
        }

        // try existing pools with matching slot_size first
        for (i, pool) in self.slot_pools.iter().enumerate().rev() {
            if pool.slot_size == slot_size
                && let Some(slot_ptr) = pool.alloc_slot()
            {
                self.alloc_cache[sc_idx].set(i);
                // SAFETY: slot_ptr was successfully allocated for this size class
                return unsafe {
                    let dst = slot_ptr.as_ptr() as *mut PoolItem<T>;
                    dst.write(PoolItem(value));
                    Ok(PoolPointer::from_raw(NonNull::new_unchecked(dst)))
                };
            }
        }

        // need a new pool for this size class
        // try the recycle list first
        // to avoid a round trip through the OS allocator
        if let Some(pos) = self
            .recycled_pools
            .iter()
            .rposition(|p| p.slot_size == slot_size)
        {
            let pool = self.recycled_pools.swap_remove(pos);
            // pool.reset() was already called in drop_empty_pools when it was parked
            let slot_ptr = pool.alloc_slot().ok_or(PoolAllocError::OutOfMemory)?;
            let insert_idx = self.slot_pools.len();
            // insert new pool into sorted index
            let (base, end) = pool.slot_range();
            let spos = self.sorted_ranges.partition_point(|&(b, _, _)| b < base);
            self.sorted_ranges.insert(spos, (base, end, insert_idx));
            self.slot_pools.push(pool);
            self.alloc_cache[sc_idx].set(insert_idx);

            // SAFETY: slot_ptr was successfully allocated for this size class
            return unsafe {
                let dst = slot_ptr.as_ptr() as *mut PoolItem<T>;
                dst.write(PoolItem(value));
                Ok(PoolPointer::from_raw(NonNull::new_unchecked(dst)))
            };
        }

        // Recycle list had no match, allocate a fresh page from the OS.
        let total = self.page_size.max(slot_size * 4);
        let new_pool = SlotPool::try_init(slot_size, total, 16)?;
        self.current_heap_size += new_pool.layout.size();
        let slot_ptr = new_pool.alloc_slot().ok_or(PoolAllocError::OutOfMemory)?;
        let insert_idx = self.slot_pools.len();
        // insert new pool into sorted index
        let (base, end) = new_pool.slot_range();
        let spos = self.sorted_ranges.partition_point(|&(b, _, _)| b < base);
        self.sorted_ranges.insert(spos, (base, end, insert_idx));
        self.slot_pools.push(new_pool);
        self.alloc_cache[sc_idx].set(insert_idx);

        // SAFETY: slot_ptr was successfully allocated for this size class
        unsafe {
            let dst = slot_ptr.as_ptr() as *mut PoolItem<T>;
            dst.write(PoolItem(value));
            Ok(PoolPointer::from_raw(NonNull::new_unchecked(dst)))
        }
    }

    /// drops the value at `ptr` and returns the slot to the allocator
    ///
    /// # Safety
    /// `ptr` must be a live `PoolItem<T>` allocated by this allocator,
    /// must not be used after this call
    #[inline]
    pub unsafe fn free_slot_typed<T>(&mut self, ptr: NonNull<PoolItem<T>>) {
        // SAFETY: guaranteed by caller
        unsafe { core::ptr::drop_in_place(ptr.as_ptr()) };
        self.free_slot(ptr.cast::<u8>());
    }

    #[inline]
    pub fn free_slot(&mut self, ptr: NonNull<u8>) {
        // Fast path: check if the pointer belongs to the last used pool
        let cached = self.free_cache.get();
        if cached < self.slot_pools.len() && self.slot_pools[cached].owns(ptr) {
            self.slot_pools[cached].free_slot(ptr);
            return;
        }

        // O(log n) binary search via the sorted address range index
        if let Some(pool_idx) = self.find_pool_idx(ptr) {
            self.slot_pools[pool_idx].free_slot(ptr);
            self.free_cache.set(pool_idx);
            return;
        }
        debug_assert!(
            false,
            "free_slot called with pointer {ptr:p} not owned by any slot pool; \
             possible double-free or pointer from a raw page"
        );
    }

    /// bump allocate raw bytes onto a BumpPage
    pub fn try_alloc_bytes(&mut self, layout: Layout) -> Result<NonNull<[u8]>, PoolAllocError> {
        // try the most recent bump page first
        if let Some(page) = self.bump_pages.last()
            && let Ok(ptr) = page.try_alloc(layout)
        {
            return Ok(ptr);
        }
        // allocate a new bump page with margin for padding
        let margin = 64;
        let total = self.page_size.max(layout.size() + layout.align() + margin);
        let max_align = layout.align().max(16);
        let page = BumpPage::try_init(total, max_align)?;
        self.current_heap_size += page.layout.size();
        let ptr = page
            .try_alloc(layout)
            .map_err(|_| PoolAllocError::OutOfMemory)?;
        self.bump_pages.push(page);
        Ok(ptr)
    }

    /// decrement live allocation count for the page owning ptr
    pub fn dealloc_bytes(&mut self, ptr: NonNull<u8>) {
        for page in self.bump_pages.iter().rev() {
            if page.owns(ptr) {
                page.dealloc();
                return;
            }
        }
    }

    /// try to shrink a raw allocation in place
    pub fn shrink_bytes_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> bool {
        for page in self.bump_pages.iter().rev() {
            if page.owns(ptr) {
                return page.shrink_in_place(ptr, old_layout, new_layout);
            }
        }
        false
    }

    /// try to grow a raw allocation in place
    pub fn grow_bytes_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> bool {
        for page in self.bump_pages.iter().rev() {
            if page.owns(ptr) {
                return page.grow_in_place(ptr, old_layout, new_layout);
            }
        }
        false
    }

    /// Reclaim slot pool pages that became empty after a GC sweep.
    ///
    /// Empty pages are parked in a recycle list (up to `max_recycled`)
    /// to avoid global allocator round trips on the next allocation.
    pub fn drop_empty_pools(&mut self) {
        // Drain fully empty slot pools into the recycle list.
        for pool in self.slot_pools.extract_if(.., |p| p.run_drop_check()) {
            if self.recycled_pools.len() < self.max_recycled {
                pool.reset();
                self.recycled_pools.push(pool);
            } else {
                self.current_heap_size = self.current_heap_size.saturating_sub(pool.layout.size());
            }
        }

        // Bump pages have no size class affinity so we always free them.
        self.bump_pages.retain(|p| {
            if p.run_drop_check() {
                self.current_heap_size = self.current_heap_size.saturating_sub(p.layout.size());
                false
            } else {
                true
            }
        });

        // Reset all caches since pool indices are stale after extract_if.
        self.free_cache.set(usize::MAX);
        for cache in &self.alloc_cache {
            cache.set(usize::MAX);
        }

        self.rebuild_sorted_ranges();
    }
}
