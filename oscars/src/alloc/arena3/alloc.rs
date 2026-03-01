//! core arena data structures
//!
//! `Arena` is a fixed size buffer split into slots,
//! bitmap tracks occupied slots, freed slots store a `next` pointer
//! in their first 8 bytes
//!
//! overhead: 0 bytes per object

use core::{cell::Cell, marker::PhantomData, ptr::NonNull};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use crate::alloc::arena3::ArenaAllocError;

// transparent wrapper around a GC value
// liveness is tracked by the arena bitmap.
#[derive(Debug)]
#[repr(transparent)]
pub struct ArenaHeapItem<T: ?Sized>(pub T);

impl<T: ?Sized> ArenaHeapItem<T> {
    pub fn value(&self) -> &T {
        &self.0
    }

    pub fn value_mut(&mut self) -> &mut T {
        &mut self.0
    }

    pub fn as_ptr(&mut self) -> *mut T {
        &mut self.0 as *mut T
    }
}

// type erased pointer into an arena slot
// points to the start of the slot.
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
}

// typed pointer into an arena slot
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
}

impl core::fmt::Debug for Arena {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Arena")
            .field("slot_size", &self.slot_size)
            .field("slot_count", &self.slot_count)
            .field("layout", &self.layout)
            .field("bitmap_words", &self.bitmap_words)
            .field("bump", &self.bump.get())
            .field("live", &self.live.get())
            .field("active_raw_allocs", &self.active_raw_allocs.get())
            .finish()
    }
}

// fixed size bump-allocator with bitmap tracking and an embedded free list
//
// buffer: `[ bitmap ][ slots ]`
// bitmap bit `i` is 1 when occupied
pub struct Arena {
    pub(crate) slot_size: usize,
    pub(crate) slot_count: usize,
    pub(crate) layout: Layout,
    pub(crate) buffer: NonNull<u8>,
    pub(crate) bitmap_words: usize,
    pub(crate) bump: Cell<usize>,
    pub(crate) free_list: Cell<*mut u8>,
    pub(crate) live: Cell<usize>,
    pub(crate) active_raw_allocs: Cell<usize>,
}

// SAFETY: `Arena` is used only from a single threaded GC context
unsafe impl Send for Arena {}

impl Arena {
    pub fn try_init(
        slot_size: usize,
        total_capacity: usize,
        max_align: usize,
    ) -> Result<Self, ArenaAllocError> {
        assert!(
            slot_size >= 8,
            "slot_size must be >= 8 (for free-list pointer)"
        );

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
            free_list: Cell::new(core::ptr::null_mut()),
            live: Cell::new(0),
            active_raw_allocs: Cell::new(0),
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

    // prevent objects without GcHeader from being swept
    pub fn mark_slot(&self, ptr: NonNull<u8>) {
        let idx = self.slot_index(ptr);
        self.bitmap_set(idx);
    }

    pub fn is_marked(&self, ptr: NonNull<u8>) -> bool {
        let i = self.slot_index(ptr);
        // SAFETY: pointer addition and cast are within the bitmap bounds
        let word = unsafe { &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>) };
        (word.get() & (1u64 << (i % 64))) != 0
    }

    // allocate a typed slot. returns None if full.
    pub fn alloc_slot(&self) -> Option<NonNull<u8>> {
        // pop from free list if available
        let fl = self.free_list.get();
        if !fl.is_null() {
            // SAFETY: reading next pointer from a slot that was previously freed
            let next = unsafe { (fl as *const *mut u8).read() };
            self.free_list.set(next);

            // SAFETY: free list pointer is checked against null
            let nn = unsafe { NonNull::new_unchecked(fl) };
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

    // release a slot back to the free list.
    pub fn free_slot(&self, ptr: NonNull<u8>) {
        let idx = self.slot_index(ptr);
        self.bitmap_clear(idx);
        // SAFETY: writing next pointer to the start of the freed slot
        unsafe {
            (ptr.as_ptr() as *mut *mut u8).write(self.free_list.get());
        }
        self.free_list.set(ptr.as_ptr());
        self.live.set(self.live.get().saturating_sub(1));
    }

    // try to allocate raw bytes. tracked only via active_raw_allocs.
    // placed after the bitmap section to prevent corruption.
    pub fn try_alloc_bytes(&self, layout: Layout) -> Result<NonNull<[u8]>, ArenaAllocError> {
        let size = layout.size();
        let align = layout.align();

        if align > self.layout.align() {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        let base = self.bitmap_bytes().max(self.bump.get());
        // SAFETY: base is within buffer bounds
        let current_ptr = unsafe { self.buffer.as_ptr().add(base) };
        let padding = current_ptr.align_offset(align);
        if padding == usize::MAX {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }
        let offset = base + padding;
        if offset + size > self.layout.size() {
            return Err(ArenaAllocError::OutOfMemory);
        }

        self.bump.set(offset + size);
        self.active_raw_allocs.set(self.active_raw_allocs.get() + 1);

        // SAFETY: offset is within buffer bounds and derived from a NonNull base
        let ptr = unsafe { NonNull::new_unchecked(self.buffer.as_ptr().add(offset)) };
        Ok(NonNull::slice_from_raw_parts(ptr, size))
    }

    pub fn dealloc_bytes(&self) {
        self.active_raw_allocs
            .set(self.active_raw_allocs.get().saturating_sub(1));
    }

    // returns true if drop is safe (all slots free & no raw allocs)
    pub fn run_drop_check(&self) -> bool {
        if self.active_raw_allocs.get() > 0 {
            return false;
        }
        if self.live.get() > 0 {
            return false;
        }
        for word_idx in 0..self.bitmap_words {
            // SAFETY: reading bitmap words is within buffer bounds
            let word = unsafe { (self.buffer.as_ptr().add(word_idx * 8) as *const u64).read() };
            if word != 0 {
                return false;
            }
        }
        true
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // SAFETY: buffer was allocated with the same layout by the global allocator
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}
