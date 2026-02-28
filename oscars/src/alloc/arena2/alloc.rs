//! Core arena data structures: `Arena`, `ArenaHeapItem`, and pointer types.
//!
//! Design: each `Arena` is a fixed-size buffer divided into equal-sized slots.
//! A bitmap (embedded at the top of the buffer) tracks which slots are
//! occupied.  Freed slots re-use their first 8 bytes as an embedded free-list
//! `next` pointer.  This is the SpiderMonkey zone-allocator model.
//!
//! Per-object overhead: 0 bytes (bitmap + free list are out-of-band).

use core::{
    cell::Cell,
    marker::PhantomData,
    ptr::NonNull,
};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use crate::alloc::arena2::ArenaAllocError;

// ---------------------------------------------------------------------------
// ArenaHeapItem — zero-cost transparent wrapper
// ---------------------------------------------------------------------------

/// Zero-overhead wrapper around a GC-managed value.
///
/// Previously contained a `next: TaggedPtr` field for linked-list liveness
/// tracking.  Liveness is now tracked by the arena bitmap, so this wrapper
/// is purely structural (it lets the API express "this came from an Arena").
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

// ---------------------------------------------------------------------------
// ErasedArenaPointer / ArenaPointer
// ---------------------------------------------------------------------------

/// A type-erased pointer into an arena slot.
///
/// The inner `NonNull<u8>` points to the **start of the slot** (i.e. the
/// beginning of an `ArenaHeapItem<T>` for whatever `T` was stored).
/// The lifetime `'arena` is a marker that prevents the pointer outliving the
/// `ArenaAllocator` that owns it.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ErasedArenaPointer<'arena>(NonNull<u8>, PhantomData<&'arena ()>);

impl<'arena> ErasedArenaPointer<'arena> {
    pub(crate) fn from_raw(raw: NonNull<u8>) -> Self {
        Self(raw, PhantomData)
    }

    pub fn as_ptr(&self) -> NonNull<u8> {
        self.0
    }

    /// Re-type this pointer to a concrete `ArenaPointer<'arena, T>`.
    ///
    /// # Safety
    ///
    /// `T` must be the type that was originally allocated into this slot.
    pub unsafe fn to_typed_arena_pointer<T>(self) -> ArenaPointer<'arena, T> {
        ArenaPointer(self.0.cast::<ArenaHeapItem<T>>(), PhantomData)
    }

    // Compatibility shim used in a few places that called `.as_non_null()`.
    pub fn as_non_null(&self) -> NonNull<u8> {
        self.0
    }
}

/// A typed pointer into an arena slot.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ArenaPointer<'arena, T>(
    NonNull<ArenaHeapItem<T>>,
    PhantomData<&'arena T>,
);

impl<'arena, T> ArenaPointer<'arena, T> {
    pub(crate) unsafe fn from_raw(raw: NonNull<ArenaHeapItem<T>>) -> Self {
        Self(raw, PhantomData)
    }

    pub fn as_inner_ref(&self) -> &'arena T {
        unsafe { &(*self.0.as_ptr()).0 }
    }

    pub fn as_ptr(&self) -> NonNull<ArenaHeapItem<T>> {
        self.0
    }

    pub fn to_erased(self) -> ErasedArenaPointer<'arena> {
        ErasedArenaPointer(self.0.cast::<u8>(), PhantomData)
    }
}

// ---------------------------------------------------------------------------
// Arena
// ---------------------------------------------------------------------------

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

/// A fixed-size bump-allocator with bitmap liveness tracking and an embedded
/// free list for slot reuse.
///
/// ## Buffer layout
///
/// ```text
/// [ bitmap: bitmap_words × 8 bytes ][ slot_0 ][ slot_1 ] ... [ slot_N ]
/// ```
///
/// Bit `i` in the bitmap is 1 when slot `i` is occupied, 0 when free.
///
/// ## Free list
///
/// When a slot is freed the first 8 bytes of the slot memory are used to
/// store a pointer to the next free slot (or null).  The arena keeps a
/// `free_list` cell pointing at the most-recently freed slot.
pub struct Arena {
    /// Size of each typed GC slot in bytes.
    pub(crate) slot_size: usize,
    /// Total number of slots in this arena.
    pub(crate) slot_count: usize,
    /// Layout passed to the global allocator for the buffer.
    pub(crate) layout: Layout,
    /// Raw backing buffer: `[bitmap_words × 8][slot_0..slot_N]`.
    pub(crate) buffer: NonNull<u8>,
    /// Number of `u64` words in the bitmap section.
    pub(crate) bitmap_words: usize,
    /// Next uninitialized slot index (0 = first slot, slot_count = full).
    pub(crate) bump: Cell<usize>,
    /// Head of embedded free list (`null` = empty list).
    pub(crate) free_list: Cell<*mut u8>,
    /// Number of currently occupied (live) slots.
    pub(crate) live: Cell<usize>,
    /// Number of outstanding raw (`Allocator::allocate`) byte allocations.
    pub(crate) active_raw_allocs: Cell<usize>,
}

// SAFETY: `Arena` is used only from a single-threaded GC context.
unsafe impl Send for Arena {}

impl Arena {
    /// Try to initialise a new arena for objects of `slot_size` bytes.
    ///
    /// `total_capacity` is the total buffer size in bytes (including the
    /// bitmap section).  `max_align` is the alignment the buffer must satisfy.
    pub fn try_init(
        slot_size: usize,
        total_capacity: usize,
        max_align: usize,
    ) -> Result<Self, ArenaAllocError> {
        assert!(slot_size >= 8, "slot_size must be at least 8 bytes (free-list needs a pointer)");

        // Estimate slot_count to size the bitmap, then compute exact values.
        // bitmap_words = ceil(slot_count / 64)
        // bitmap_bytes = bitmap_words * 8
        // slot section = total_capacity - bitmap_bytes
        let estimated_slots = total_capacity / slot_size;
        let bitmap_words = (estimated_slots + 63) / 64;
        let bitmap_bytes = bitmap_words * 8;
        let slot_area = total_capacity.saturating_sub(bitmap_bytes);
        let slot_count = slot_area / slot_size;

        let layout = Layout::from_size_align(total_capacity, max_align)
            .map_err(ArenaAllocError::LayoutError)?;

        let buffer = unsafe {
            let ptr = alloc(layout);
            let Some(nn) = NonNull::new(ptr) else {
                handle_alloc_error(layout)
            };
            nn
        };

        // Zero-initialise the bitmap so all slots are reported as free.
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

    // -----------------------------------------------------------------------
    // Bitmap helpers
    // -----------------------------------------------------------------------

    /// Number of bytes the bitmap occupies at the base of the buffer.
    #[inline]
    pub(crate) fn bitmap_bytes(&self) -> usize {
        self.bitmap_words * 8
    }

    /// Pointer to the start of the slot array (just after the bitmap).
    #[inline]
    fn slot_base(&self) -> *mut u8 {
        unsafe { self.buffer.as_ptr().add(self.bitmap_bytes()) }
    }

    /// Raw pointer to slot `i`.
    #[inline]
    pub(crate) fn slot_ptr(&self, i: usize) -> NonNull<u8> {
        let ptr = unsafe { self.slot_base().add(i * self.slot_size) };
        unsafe { NonNull::new_unchecked(ptr) }
    }

    /// Compute slot index from a pointer that was returned by `alloc_slot`.
    #[inline]
    pub(crate) fn slot_index(&self, ptr: NonNull<u8>) -> usize {
        let base = self.slot_base() as usize;
        let addr = ptr.as_ptr() as usize;
        (addr - base) / self.slot_size
    }

    /// Returns true if this arena's buffer owns `ptr`.
    pub(crate) fn owns(&self, ptr: NonNull<u8>) -> bool {
        let buf_start = self.slot_base() as usize;
        let buf_end = buf_start + self.slot_count * self.slot_size;
        let addr = ptr.as_ptr() as usize;
        addr >= buf_start && addr < buf_end
    }

    /// Mark slot `i` occupied in the bitmap.
    #[inline]
    fn bitmap_set(&self, i: usize) {
        let word = unsafe {
            &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>)
        };
        word.set(word.get() | (1u64 << (i % 64)));
    }

    /// Mark slot `i` free in the bitmap.
    #[inline]
    fn bitmap_clear(&self, i: usize) {
        let word = unsafe {
            &*(self.buffer.as_ptr().add((i / 64) * 8) as *const Cell<u64>)
        };
        word.set(word.get() & !(1u64 << (i % 64)));
    }

    /// Return true if slot `i` is occupied in the bitmap.
    #[inline]
    pub(crate) fn bitmap_is_set(&self, i: usize) -> bool {
        let word = unsafe {
            (self.buffer.as_ptr().add((i / 64) * 8) as *const u64).read()
        };
        word >> (i % 64) & 1 == 1
    }

    // -----------------------------------------------------------------------
    // Allocation / deallocation
    // -----------------------------------------------------------------------

    /// Try to allocate one typed-GC slot.
    ///
    /// Returns the slot pointer on success.  Returns `None` when all slots are
    /// full (caller must create a new arena).
    pub fn alloc_slot(&self) -> Option<NonNull<u8>> {
        // Fast path: pop from free list.
        let fl = self.free_list.get();
        if !fl.is_null() {
            // Read next pointer embedded at the start of the freed slot.
            let next = unsafe { (fl as *const *mut u8).read() };
            self.free_list.set(next);

            let nn = unsafe { NonNull::new_unchecked(fl) };
            let idx = self.slot_index(nn);
            self.bitmap_set(idx);
            self.live.set(self.live.get() + 1);
            return Some(nn);
        }

        // Slow path: bump-allocate a fresh slot.
        let idx = self.bump.get();
        if idx >= self.slot_count {
            return None; // OOM
        }
        self.bump.set(idx + 1);
        let ptr = self.slot_ptr(idx);
        self.bitmap_set(idx);
        self.live.set(self.live.get() + 1);
        Some(ptr)
    }

    /// Release a typed-GC slot back to this arena's free list.
    ///
    /// Clears the bitmap bit, embeds the next-free-slot pointer inside the
    /// slot memory, and decrements the live count.
    pub fn free_slot(&self, ptr: NonNull<u8>) {
        let idx = self.slot_index(ptr);
        self.bitmap_clear(idx);
        // Embed next pointer at the start of the freed slot.
        unsafe {
            (ptr.as_ptr() as *mut *mut u8).write(self.free_list.get());
        }
        self.free_list.set(ptr.as_ptr());
        self.live.set(self.live.get().saturating_sub(1));
    }

    /// Try to allocate raw bytes (for `Allocator::allocate`).
    ///
    /// Raw allocations are tracked only via `active_raw_allocs`.  They share
    /// the same buffer as typed-GC arenas but start after the bitmap section
    /// so they do not corrupt the bitmap that `run_drop_check` reads.
    pub fn try_alloc_bytes(&self, layout: Layout) -> Result<NonNull<[u8]>, ArenaAllocError> {
        let size = layout.size();
        let align = layout.align();

        if align > self.layout.align() {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        // Raw allocations start at `bitmap_bytes`, not at 0.  This ensures
        // raw data never overwrites the bitmap section that `run_drop_check`
        // reads.  `bump` begins at 0 and is advanced to `bitmap_bytes` on the
        // first raw alloc if it hasn't already been advanced by a prior
        // typed-GC alloc.
        let base = self.bitmap_bytes().max(self.bump.get());
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

        let ptr = unsafe { NonNull::new_unchecked(self.buffer.as_ptr().add(offset)) };
        Ok(NonNull::slice_from_raw_parts(ptr, size))
    }

    /// Bytes consumed by raw allocations (stored in bump when slot_size == 0).
    fn live_bytes_raw(&self) -> usize {
        self.bump.get()
    }

    /// Decrement the raw-allocations counter for this arena.
    pub fn dealloc_bytes(&self) {
        self.active_raw_allocs
            .set(self.active_raw_allocs.get().saturating_sub(1));
    }

    // -----------------------------------------------------------------------
    // Drop check (O(bitmap_words) instead of O(linked list))
    // -----------------------------------------------------------------------

    /// Returns `true` when every slot is free and no raw allocations are
    /// outstanding — i.e. the arena can safely be dropped.
    ///
    /// Cost: O(`bitmap_words`) = O(slot_count / 64).
    pub fn run_drop_check(&self) -> bool {
        if self.active_raw_allocs.get() > 0 {
            return false;
        }
        if self.live.get() > 0 {
            return false;
        }
        // Belt-and-suspenders: also verify the bitmap.
        for word_idx in 0..self.bitmap_words {
            let word = unsafe {
                (self.buffer.as_ptr().add(word_idx * 8) as *const u64).read()
            };
            if word != 0 {
                return false;
            }
        }
        result
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}
