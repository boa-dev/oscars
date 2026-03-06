use core::{cell::Cell, marker::PhantomData, ptr::NonNull};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use crate::alloc::arena3::ArenaAllocError;

// ree slot pointing to the next free slot
// `repr(C)` puts `next` exactly at the start of the slot
#[repr(C)]
pub(crate) struct FreeSlot {
    next: *mut FreeSlot,
}

// transparent wrapper around a GC value
// liveness is tracked by the pool bitmap
#[derive(Debug)]
#[repr(transparent)]
pub struct ArenaHeapItem<T: ?Sized>(pub T);

impl<T: ?Sized> ArenaHeapItem<T> {
    pub fn value(&self) -> &T {
        &self.0
    }

    pub fn as_ptr(&mut self) -> *mut T {
        &mut self.0 as *mut T
    }
}

// type erased pointer into a pool slot
// `'arena` prevents outliving the allocator
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ErasedArenaPointer<'arena>(NonNull<u8>, PhantomData<&'arena ()>);

impl<'arena> ErasedArenaPointer<'arena> {
    pub fn as_ptr(&self) -> NonNull<u8> {
        self.0
    }

    // retype this pointer
    // SAFETY: caller must ensure `T` matches the original allocation
    pub unsafe fn to_typed_arena_pointer<T>(self) -> ArenaPointer<'arena, T> {
        ArenaPointer(self.0.cast::<ArenaHeapItem<T>>(), PhantomData)
    }

    pub fn as_non_null(&self) -> NonNull<u8> {
        self.0
    }

    // extend the lifetime of this erased arena pointer to 'static
    //
    // SAFETY: same as ArenaPointer::extend_lifetime
    pub(crate) unsafe fn extend_lifetime(self) -> ErasedArenaPointer<'static> {
        ErasedArenaPointer(self.0, PhantomData)
    }
}

// typed pointer into a pool slot
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ArenaPointer<'arena, T>(NonNull<ArenaHeapItem<T>>, PhantomData<&'arena T>);

impl<'arena, T> ArenaPointer<'arena, T> {
    pub(crate) unsafe fn from_raw(raw: NonNull<ArenaHeapItem<T>>) -> Self {
        Self(raw, PhantomData)
    }

    pub fn as_inner_ref(&self) -> &'arena T {
        // SAFETY: pointer is valid and properly aligned
        unsafe { &(*self.0.as_ptr()).0 }
    }

    pub fn as_ptr(&self) -> NonNull<ArenaHeapItem<T>> {
        self.0
    }

    pub fn to_erased(self) -> ErasedArenaPointer<'arena> {
        ErasedArenaPointer(self.0.cast::<u8>(), PhantomData)
    }

    // SAFETY: safe because the gc collector owns the arena and keeps it alive
    pub(crate) unsafe fn extend_lifetime(self) -> ArenaPointer<'static, T> {
        ArenaPointer(self.0, PhantomData)
    }
}

/// SlotPool ///

impl core::fmt::Debug for SlotPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlotPool")
            .field("slot_size", &self.slot_size)
            .field("slot_count", &self.slot_count)
            .field("layout", &self.layout)
            .field("bitmap_words", &self.bitmap_words)
            .field("bump", &self.bump.get())
            .field("live", &self.live.get())
            .finish()
    }
}

// fixed size slot pool with the layout: `[ bitmap ][ slots ]`
// bitmap tracks live slots, freed slots form a linked list to be reused
//
pub(crate) struct SlotPool {
    pub(crate) slot_size: usize,
    pub(crate) slot_count: usize,
    pub(crate) layout: Layout,
    pub(crate) buffer: NonNull<u8>,
    pub(crate) bitmap_words: usize,
    pub(crate) bump: Cell<usize>,
    // head of the free list, None when empty
    pub(crate) free_list: Cell<Option<NonNull<FreeSlot>>>,
    // occupied slot count, kept in sync with the bitmap by alloc_slot/free_slot
    pub(crate) live: Cell<usize>,
}

impl SlotPool {
    pub fn try_init(
        slot_size: usize,
        total_capacity: usize,
        max_align: usize,
    ) -> Result<Self, ArenaAllocError> {
        assert!(
            slot_size >= core::mem::size_of::<FreeSlot>(),
            "slot_size must fit a FreeSlot (needed for the intrusive free list)"
        );

        // guess the slot count (ignoring bitmap size), size the bitmap based on that guess
        // (rounded up to 64 bit words), then subtract the bitmap size from the total capacity to get the real slot count
        // example (512 capacity, 16 slot size): guess 32 slots -> 8 byte bitmap, real 504 bytes left -> 31 slots
        // layout: [ 8-byte bitmap ][ 31 x 16-byte slots ] = 504 bytes used
        let estimated = total_capacity / slot_size;
        let bitmap_words = (estimated + 63) / 64;
        let bitmap_bytes = bitmap_words * 8;
        let slot_area = total_capacity.saturating_sub(bitmap_bytes);
        let slot_count = slot_area / slot_size;

        let layout = Layout::from_size_align(total_capacity, max_align)
            .map_err(ArenaAllocError::LayoutError)?;

        // SAFETY: allocating with a valid Layout
        let buffer = unsafe {
            let ptr = alloc(layout);
            let Some(nn) = NonNull::new(ptr) else {
                handle_alloc_error(layout)
            };
            nn
        };

        // zero the bitmap
        // SAFETY: buffer is valid for at least `bitmap_bytes`
        unsafe {
            core::ptr::write_bytes(buffer.as_ptr(), 0, bitmap_bytes);
        }

        Ok(Self {
            slot_size,
            slot_count,
            layout,
            buffer,
            bitmap_words,
            bump: Cell::new(0),
            free_list: Cell::new(None),
            live: Cell::new(0),
        })
    }

    #[inline]
    pub(crate) fn bitmap_bytes(&self) -> usize {
        self.bitmap_words * 8
    }

    #[inline]
    fn slot_base(&self) -> *mut u8 {
        // SAFETY: adding bitmap_bytes is within the buffer bounds
        unsafe { self.buffer.as_ptr().add(self.bitmap_bytes()) }
    }

    #[inline]
    pub(crate) fn slot_ptr(&self, i: usize) -> NonNull<u8> {
        // SAFETY: adding i * slot_size is within the buffer bounds
        let ptr = unsafe { self.slot_base().add(i * self.slot_size) };
        // SAFETY: ptr is derived from a NonNull base and cannot be null
        unsafe { NonNull::new_unchecked(ptr) }
    }

    #[inline]
    pub(crate) fn slot_index(&self, ptr: NonNull<u8>) -> usize {
        let base = self.slot_base() as usize;
        let addr = ptr.as_ptr() as usize;
        (addr - base) / self.slot_size
    }

    pub(crate) fn owns(&self, ptr: NonNull<u8>) -> bool {
        let buf_start = self.slot_base() as usize;
        let buf_end = buf_start + self.slot_count * self.slot_size;
        let addr = ptr.as_ptr() as usize;
        addr >= buf_start && addr < buf_end
    }

    #[inline]
    fn bitmap_set(&self, i: usize) {
        // SAFETY: pointer addition and cast are within the bitmap bounds
        let word = unsafe { &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>) };
        word.set(word.get() | (1u64 << (i % 64)));
    }

    #[inline]
    fn bitmap_clear(&self, i: usize) {
        // SAFETY: pointer addition and cast are within the bitmap bounds
        let word = unsafe { &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>) };
        word.set(word.get() & !(1u64 << (i % 64)));
    }

    // mark the slot as occupied outside of alloc_slot
    pub fn mark_slot(&self, ptr: NonNull<u8>) {
        let idx = self.slot_index(ptr);
        self.bitmap_set(idx);
    }

    // returns true if the slot at `ptr` is marked as occupied in the bitmap
    //
    // TODO: for the planned bitmap based sweep, unused until then
    #[allow(dead_code)]
    pub fn is_marked(&self, ptr: NonNull<u8>) -> bool {
        let i = self.slot_index(ptr);
        // SAFETY: pointer addition and cast are within the bitmap bounds
        let word = unsafe { &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>) };
        (word.get() & (1u64 << (i % 64))) != 0
    }

    // allocate a slot, returns None if full.
    pub fn alloc_slot(&self) -> Option<NonNull<u8>> {
        // pop from free list if available
        if let Some(head) = self.free_list.get() {
            // SAFETY: `head` points to a FreeSlot we wrote in free_slot
            // reading `next` is safe while the slot is in the free list
            let next = unsafe { (*head.as_ptr()).next };
            self.free_list.set(NonNull::new(next));

            let nn = head.cast::<u8>();
            let idx = self.slot_index(nn);
            self.bitmap_set(idx);
            self.live.set(self.live.get() + 1);
            return Some(nn);
        }

        let idx = self.bump.get();
        if idx >= self.slot_count {
            return None;
        }
        self.bump.set(idx + 1);
        let ptr = self.slot_ptr(idx);
        self.bitmap_set(idx);
        self.live.set(self.live.get() + 1);
        Some(ptr)
    }

    // return a slot to the free list
    pub fn free_slot(&self, ptr: NonNull<u8>) {
        let idx = self.slot_index(ptr);
        self.bitmap_clear(idx);
        // SAFETY: slot is large enough to hold a FreeSlot,
        // we reinterpret the slot's memory as a free list node.
        unsafe {
            let node = ptr.cast::<FreeSlot>();
            node.as_ptr().write(FreeSlot {
                next: self
                    .free_list
                    .get()
                    .map(NonNull::as_ptr)
                    .unwrap_or(core::ptr::null_mut()),
            });
            self.free_list.set(Some(node));
        }
        self.live.set(self.live.get().saturating_sub(1));
    }

    // returns true when the pool is empty and safe to drop
    // `live` tracks the count, so no bitmap scan is needed
    pub fn run_drop_check(&self) -> bool {
        self.live.get() == 0
    }
}

impl Drop for SlotPool {
    fn drop(&mut self) {
        // SAFETY: buffer was allocated with the same layout by the global allocator
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}

/// BumpPage ///

// pure bump allocator for raw bytes with a linear pointer over a buffer
// no per allocation tracking, the whole page is dropped when empty
#[derive(Debug)]
pub(crate) struct BumpPage {
    pub(crate) layout: Layout,
    pub(crate) buffer: NonNull<u8>,
    pub(crate) bump: Cell<usize>,
    // number of live allocations on this page, when hits 0 the page
    // is eligible for reclamation by drop_dead_arenas
    pub(crate) active_allocs: Cell<usize>,
}

impl BumpPage {
    pub fn try_init(total_capacity: usize, max_align: usize) -> Result<Self, ArenaAllocError> {
        let layout = Layout::from_size_align(total_capacity, max_align)
            .map_err(ArenaAllocError::LayoutError)?;

        // SAFETY: allocating with a valid Layout
        let buffer = unsafe {
            let ptr = alloc(layout);
            let Some(nn) = NonNull::new(ptr) else {
                handle_alloc_error(layout)
            };
            nn
        };

        Ok(Self {
            layout,
            buffer,
            bump: Cell::new(0),
            active_allocs: Cell::new(0),
        })
    }

    pub fn try_alloc(&self, layout: Layout) -> Result<NonNull<[u8]>, ArenaAllocError> {
        let size = layout.size();
        let align = layout.align();

        if align > self.layout.align() {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        // SAFETY: bump is within buffer bounds
        let current_ptr = unsafe { self.buffer.as_ptr().add(self.bump.get()) };
        let padding = current_ptr.align_offset(align);
        if padding == usize::MAX {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }
        let offset = self.bump.get() + padding;
        if offset + size > self.layout.size() {
            return Err(ArenaAllocError::OutOfMemory);
        }

        self.bump.set(offset + size);
        self.active_allocs.set(self.active_allocs.get() + 1);

        // SAFETY: offset is within buffer bounds and derived from a NonNull base
        let ptr = unsafe { NonNull::new_unchecked(self.buffer.as_ptr().add(offset)) };
        Ok(NonNull::slice_from_raw_parts(ptr, size))
    }

    // decrements the live allocation count
    // the page is freed by `drop_dead_arenas` when active_allocs hits zero
    pub fn dealloc(&self) {
        self.active_allocs
            .set(self.active_allocs.get().saturating_sub(1));
    }

    pub fn owns(&self, ptr: NonNull<u8>) -> bool {
        let start = self.buffer.as_ptr() as usize;
        let end = start + self.layout.size();
        let addr = ptr.as_ptr() as usize;
        addr >= start && addr < end
    }

    // try to shrink the most recent allocation in place by rewinding the bump
    pub fn shrink_in_place(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> bool {
        let offset = ptr.as_ptr() as usize - self.buffer.as_ptr() as usize;
        if offset + old_layout.size() == self.bump.get() {
            self.bump.set(offset + new_layout.size());
            true
        } else {
            false
        }
    }

    // try to grow the most recent allocation in place by extending the bump
    pub fn grow_in_place(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> bool {
        let offset = ptr.as_ptr() as usize - self.buffer.as_ptr() as usize;
        if offset + old_layout.size() == self.bump.get() {
            let new_end = offset + new_layout.size();
            if new_end <= self.layout.size() {
                self.bump.set(new_end);
                return true;
            }
        }
        false
    }

    // returns true when all allocations on this page have been released.
    pub fn run_drop_check(&self) -> bool {
        self.active_allocs.get() == 0
    }
}

impl Drop for BumpPage {
    fn drop(&mut self) {
        // SAFETY: buffer was allocated with the same layout by the global allocator
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}
