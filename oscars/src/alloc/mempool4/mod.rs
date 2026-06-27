//! Allocations return a [`Gc<'_, T>`] wrapping a [`CustomPtr`] `(pool_id, slot_idx)`
//! Values are read back through [`PoolAllocator4::mutate`] -> [`AllocCtx::resolve`]
//! The heap can be saved and restored with [`serialize()`] / [`deserialize()`]

use core::{cell::Cell, marker::PhantomData, ptr::NonNull};
use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use rust_alloc::vec::Vec;

mod ptr;
mod serialize;

#[cfg(test)]
mod tests;

pub use ptr::{CustomPtr, Gc, MAX_POOL_ID, MAX_SLOT_IDX};
pub use serialize::{DeserializeError, deserialize, serialize};

// errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolAllocError4 {
    OutOfMemory,
    LayoutError,
    PoolIdExhausted,
    /// `(pool_id, slot_idx)` can't fit in 32 bits
    PointerOverflow,
}

const SIZE_CLASSES: &[usize] = &[16, 24, 32, 48, 64, 96, 128, 192, 256, 512, 1024, 2048];

#[inline(always)]
fn size_class_for(size: usize) -> usize {
    SIZE_CLASSES
        .partition_point(|&sc| sc < size)
        .min(SIZE_CLASSES.len() - 1)
}

const DEFAULT_PAGE_BYTES: usize = 65_536;

#[repr(C)]
struct FreeSlot {
    next: *mut FreeSlot,
}

/// fixed size slot pool. buffer layout: `[ bitmap ][ slot_0 | slot_1 | ... ]`
pub struct Pool4 {
    pub(crate) pool_id: u32,
    pub(crate) slot_size: usize,
    pub(crate) slot_count: usize,
    pub(crate) layout: Layout,
    buffer: NonNull<u8>,
    bitmap_bytes: usize,
    bump: Cell<usize>,
    free_list: Cell<Option<NonNull<FreeSlot>>>,
    live: Cell<usize>,
}

impl core::fmt::Debug for Pool4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pool4")
            .field("pool_id", &self.pool_id)
            .field("slot_size", &self.slot_size)
            .field("slot_count", &self.slot_count)
            .field("live", &self.live.get())
            .finish()
    }
}

impl Pool4 {
    /// Creates a new pool, `pool_id` must be unique within the allocator.
    pub fn try_init(
        pool_id: u32,
        slot_size: usize,
        capacity: usize,
    ) -> Result<Self, PoolAllocError4> {
        assert!(
            slot_size >= core::mem::size_of::<FreeSlot>(),
            "slot_size must fit a FreeSlot"
        );

        let estimated_slot_count = capacity / slot_size;
        let bitmap_bytes = estimated_slot_count.div_ceil(64) * 8;
        let slot_area = capacity.saturating_sub(bitmap_bytes);
        let slot_count = slot_area / slot_size;

        let layout =
            Layout::from_size_align(capacity, 16).map_err(|_| PoolAllocError4::LayoutError)?;

        let buffer = unsafe {
            let ptr = alloc(layout);
            match NonNull::new(ptr) {
                Some(nn) => nn,
                None => handle_alloc_error(layout),
            }
        };

        unsafe { core::ptr::write_bytes(buffer.as_ptr(), 0, bitmap_bytes) };

        Ok(Self {
            pool_id,
            slot_size,
            slot_count,
            layout,
            buffer,
            bitmap_bytes,
            bump: Cell::new(0),
            free_list: Cell::new(None),
            live: Cell::new(0),
        })
    }

    #[inline]
    fn slot_base(&self) -> *mut u8 {
        unsafe { self.buffer.as_ptr().add(self.bitmap_bytes) }
    }

    #[inline]
    pub(crate) fn slot_ptr(&self, i: usize) -> NonNull<u8> {
        debug_assert!(i < self.slot_count);
        unsafe { NonNull::new_unchecked(self.slot_base().add(i * self.slot_size)) }
    }

    #[inline]
    fn slot_index(&self, ptr: NonNull<u8>) -> usize {
        (ptr.as_ptr() as usize - self.slot_base() as usize) / self.slot_size
    }

    #[inline]
    fn bitmap_chunk(&self, i: usize) -> &Cell<u64> {
        unsafe { &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>) }
    }

    #[inline]
    fn bitmap_set(&self, i: usize) {
        let c = self.bitmap_chunk(i);
        c.set(c.get() | (1u64 << (i % 64)));
    }

    #[inline]
    fn bitmap_clear(&self, i: usize) {
        let c = self.bitmap_chunk(i);
        c.set(c.get() & !(1u64 << (i % 64)));
    }

    /// Returns a free slot index or `None` if full.
    pub fn alloc_slot(&self) -> Option<usize> {
        if let Some(head) = self.free_list.get() {
            let next = unsafe { (*head.as_ptr()).next };
            self.free_list.set(NonNull::new(next));
            let idx = self.slot_index(head.cast::<u8>());
            self.bitmap_set(idx);
            self.live.set(self.live.get() + 1);
            return Some(idx);
        }
        let idx = self.bump.get();
        if idx >= self.slot_count {
            return None;
        }
        self.bump.set(idx + 1);
        self.bitmap_set(idx);
        self.live.set(self.live.get() + 1);
        Some(idx)
    }

    /// Returns a slot to the free list
    ///
    /// # Safety
    /// `slot_idx` must be a live slot from this pool.
    pub unsafe fn free_slot(&self, slot_idx: usize) {
        debug_assert!(slot_idx < self.slot_count);
        debug_assert!(self.live.get() > 0, "free_slot on empty pool");
        self.bitmap_clear(slot_idx);
        unsafe {
            let node = self.slot_ptr(slot_idx).cast::<FreeSlot>();
            let next = self
                .free_list
                .get()
                .map_or(core::ptr::null_mut(), |h| h.as_ptr());
            node.as_ptr().write(FreeSlot { next });
            self.free_list.set(Some(node));
        }
        self.live.set(self.live.get() - 1);
    }

    /// `true` when the pool has no live slots
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live.get() == 0
    }

    /// Number of live slots
    #[inline]
    pub fn live_count(&self) -> usize {
        self.live.get()
    }

    /// Yields the index of every live slot
    pub fn iter_live(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.slot_count).filter_map(move |i| {
            if self.bitmap_chunk(i).get() & (1u64 << (i % 64)) != 0 {
                Some(i as u32)
            } else {
                None
            }
        })
    }

    /// Raw bytes of slot `i`
    ///
    /// # Safety
    /// `slot_idx` must be live.
    pub(crate) unsafe fn slot_bytes(&self, slot_idx: usize) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.slot_ptr(slot_idx).as_ptr(), self.slot_size) }
    }
}

impl Drop for Pool4 {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) }
    }
}

/// Size-class pool allocator that returns [`Gc<'_, T>`] handles
///
/// Use [`mutate`](Self::mutate) to open a window for allocating and resolving.
pub struct PoolAllocator4 {
    pub(crate) pools: Vec<Pool4>,
    pub(crate) next_pool_id: u32,
    pub(crate) page_size: usize,
}

impl core::fmt::Debug for PoolAllocator4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PoolAllocator4")
            .field("pool_count", &self.pools.len())
            .field("next_pool_id", &self.next_pool_id)
            .finish()
    }
}

impl Default for PoolAllocator4 {
    fn default() -> Self {
        Self::new()
    }
}

impl PoolAllocator4 {
    /// Creates an empty allocator with a 64 KiB default page size
    pub fn new() -> Self {
        Self {
            pools: Vec::new(),
            next_pool_id: 0,
            page_size: DEFAULT_PAGE_BYTES,
        }
    }

    /// Sets the page size used when creating new pools
    pub fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size;
        self
    }

    // mutation window

    /// Opens a scoped mutation window. The closure receives an [`AllocCtx<'gc>`]
    /// that can hold multiple [`Gc`] handles simultaneously
    pub fn mutate<R, F>(&mut self, f: F) -> R
    where
        F: for<'gc> FnOnce(AllocCtx<'gc>) -> R,
    {
        // SAFETY: `self` is exclusively borrowed for the life of `f`.
        // The `'gc` brand prevents `AllocCtx` from escaping the closure.
        f(AllocCtx {
            inner: self as *mut Self,
            _marker: PhantomData,
        })
    }

    // raw allocation

    /// Allocates `value` and returns a `Gc<'static, T>`
    ///
    /// # Safety
    /// Prefer [`mutate`](Self::mutate). The returned `Gc` must not outlive this allocator.
    pub unsafe fn try_alloc_raw<T>(&mut self, value: T) -> Result<Gc<'static, T>, PoolAllocError4> {
        let slot_size = core::mem::size_of::<T>().max(core::mem::size_of::<FreeSlot>());
        let actual_slot_size = SIZE_CLASSES
            .get(size_class_for(slot_size))
            .copied()
            .unwrap_or(slot_size);

        for pool in self.pools.iter() {
            if pool.slot_size == actual_slot_size
                && let Some(slot_idx) = pool.alloc_slot()
            {
                let ptr = CustomPtr::new(pool.pool_id, slot_idx as u32)
                    .ok_or(PoolAllocError4::PointerOverflow)?;
                unsafe { (pool.slot_ptr(slot_idx).as_ptr() as *mut T).write(value) };
                return Ok(Gc {
                    ptr,
                    _marker: PhantomData,
                });
            }
        }

        let pool_id = self.next_pool_id;
        if pool_id > MAX_POOL_ID {
            return Err(PoolAllocError4::PoolIdExhausted);
        }
        self.next_pool_id += 1;

        let pool = Pool4::try_init(
            pool_id,
            actual_slot_size,
            self.page_size.max(actual_slot_size * 4),
        )?;
        let slot_idx = pool.alloc_slot().ok_or(PoolAllocError4::OutOfMemory)?;
        let ptr =
            CustomPtr::new(pool_id, slot_idx as u32).ok_or(PoolAllocError4::PointerOverflow)?;
        unsafe { (pool.slot_ptr(slot_idx).as_ptr() as *mut T).write(value) };
        self.pools.push(pool);

        Ok(Gc {
            ptr,
            _marker: PhantomData,
        })
    }

    /// Returns a shared reference to the value at `gc`
    #[inline]
    pub fn resolve<'gc, T>(&'gc self, gc: Gc<'gc, T>) -> &'gc T {
        let pool = self
            .find_pool(gc.ptr.pool_id())
            .expect("Gc pool_id not found in this allocator");
        unsafe { &*(pool.slot_ptr(gc.ptr.slot_idx()).as_ptr() as *const T) }
    }

    /// Returns an exclusive reference to the value at `gc`
    #[inline]
    pub fn resolve_mut<'gc, T>(&'gc mut self, gc: Gc<'gc, T>) -> &'gc mut T {
        let pool = self
            .find_pool_mut(gc.ptr.pool_id())
            .expect("Gc pool_id not found in this allocator");
        unsafe { &mut *(pool.slot_ptr(gc.ptr.slot_idx()).as_ptr() as *mut T) }
    }

    /// Drops the value at `gc` and frees the slot.
    ///
    /// # Safety
    /// `gc` must be live. Don't use the handle after this call.
    pub unsafe fn free<T>(&mut self, gc: Gc<'_, T>) {
        let pool = self
            .find_pool(gc.ptr.pool_id())
            .expect("Gc pool_id not found in this allocator");
        unsafe {
            core::ptr::drop_in_place(pool.slot_ptr(gc.ptr.slot_idx()).as_ptr() as *mut T);
            pool.free_slot(gc.ptr.slot_idx());
        }
    }

    pub fn pool_count(&self) -> usize {
        self.pools.len()
    }

    pub fn live_slot_count(&self) -> usize {
        self.pools.iter().map(|p| p.live_count()).sum()
    }

    // private

    // TODO(perf): O(n) scan; replace with a sorted index at scale.
    fn find_pool(&self, pool_id: usize) -> Option<&Pool4> {
        self.pools.iter().find(|p| p.pool_id as usize == pool_id)
    }

    fn find_pool_mut(&mut self, pool_id: usize) -> Option<&mut Pool4> {
        self.pools
            .iter_mut()
            .find(|p| p.pool_id as usize == pool_id)
    }
}

/// Scoped context from [`PoolAllocator4::mutate`]
///
/// Holds multiple [`Gc`] handles at once without borrow conflicts.
/// The `'gc` brand prevents handles from escaping the closure.
pub struct AllocCtx<'gc> {
    inner: *mut PoolAllocator4,
    _marker: PhantomData<*mut &'gc ()>,
}

impl<'gc> AllocCtx<'gc> {
    /// Allocates `value` and returns a `Gc<'gc, T>`
    pub fn try_alloc<T>(&self, value: T) -> Result<Gc<'gc, T>, PoolAllocError4> {
        unsafe { (*self.inner).try_alloc_raw(value) }
    }

    /// Returns a shared reference to the value at `gc`.
    pub fn resolve<T>(&self, gc: Gc<'gc, T>) -> &'gc T {
        let alloc = unsafe { &*self.inner };
        let pool = alloc
            .find_pool(gc.ptr.pool_id())
            .expect("Gc pool_id not found in this allocator");
        unsafe { &*(pool.slot_ptr(gc.ptr.slot_idx()).as_ptr() as *const T) }
    }

    /// Returns an exclusive reference to the value at `gc`.
    ///
    /// # Safety
    /// No other reference to the same slot may exist
    pub unsafe fn resolve_mut<T>(&self, gc: Gc<'gc, T>) -> &'gc mut T {
        // Borrow the allocator shared only to find the pool; the &mut T is
        // into the slot buffer, disjoint from the allocator struct.
        let alloc = unsafe { &*self.inner };
        let pool = alloc
            .find_pool(gc.ptr.pool_id())
            .expect("Gc pool_id not found in this allocator");
        unsafe { &mut *(pool.slot_ptr(gc.ptr.slot_idx()).as_ptr() as *mut T) }
    }

    /// Drops the value at gc and frees the slot
    ///
    /// # Safety
    /// `gc` must be live, don't use the handle after this call.
    pub unsafe fn free<T>(&self, gc: Gc<'gc, T>) {
        unsafe { (*self.inner).free(gc) }
    }

    pub fn live_slot_count(&self) -> usize {
        unsafe { (*self.inner).live_slot_count() }
    }

    pub fn pool_count(&self) -> usize {
        unsafe { (*self.inner).pool_count() }
    }
}
