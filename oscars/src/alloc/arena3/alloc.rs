use core::{alloc::LayoutError, marker::PhantomData, ptr::NonNull};
use rust_alloc::{
    alloc::{Layout, alloc, handle_alloc_error},
    vec::Vec,
};

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

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ErasedArenaPointer<'arena>(NonNull<u8>, PhantomData<&'arena ()>);

impl<'arena> ErasedArenaPointer<'arena> {
    fn from_raw(raw: NonNull<u8>) -> Self {
        Self(raw, PhantomData)
    }

    pub fn as_non_null(&self) -> NonNull<u8> {
        self.0
    }

    pub fn as_raw_ptr(&self) -> *mut u8 {
        self.0.as_ptr()
    }

    /// Returns an [`ArenaPointer`] for the current [`ErasedArenaPointer`]
    ///
    /// # Safety
    ///
    /// - `T` must be the correct type for the pointer. Casting to an invalid
    ///   type may cause undefined behavior.
    pub unsafe fn to_typed_arena_pointer<T>(self) -> ArenaPointer<'arena, T> {
        ArenaPointer(self, PhantomData)
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ArenaPointer<'arena, T>(ErasedArenaPointer<'arena>, PhantomData<&'arena T>);

impl<'arena, T> ArenaPointer<'arena, T> {
    unsafe fn from_raw(raw: NonNull<T>) -> Self {
        Self(ErasedArenaPointer::from_raw(raw.cast::<u8>()), PhantomData)
    }

    pub fn as_inner_ref(&self) -> &'arena T {
        // SAFETY: HeapItem is non-null and valid for dereferencing.
        unsafe {
            let typed_ptr = self.0.as_raw_ptr().cast::<T>();
            &(*typed_ptr)
        }
    }

    /// Return a pointer to the inner T
    ///
    /// SAFETY:
    ///
    /// - Caller must ensure that T is not dropped
    /// - Caller must ensure that the lifetime of T does not exceed it's Arena.
    pub fn as_ptr(&self) -> NonNull<T> {
        self.0.as_non_null().cast::<T>()
    }

    /// Convert the current ArenaPointer into an `ErasedArenaPointer`
    pub fn to_erased(self) -> ErasedArenaPointer<'arena> {
        self.0
    }
}

pub struct ArenaAllocationData {
    bit_index: usize,
    required_cells: usize,
}

#[derive(Debug)]
#[repr(C)]
pub struct BitmapArena<'arena> {
    pub layout: Layout,
    pub buffer: NonNull<u8>,
    pub bitmap: Vec<u64>,

    _marker: PhantomData<&'arena ()>,
}

impl<'arena> BitmapArena<'arena> {
    /// Initializes a new Arena within a provided raw buffer.
    pub fn new(arena_size: usize, max_alignment: usize) -> Result<Self, ArenaAllocError> {
        let layout = Layout::from_size_align(arena_size, max_alignment)?;
        let buffer = unsafe {
            let ptr = alloc(layout);
            let Some(data) = NonNull::new(ptr) else {
                handle_alloc_error(layout)
            };
            data
        };

        // Calculation check for 4096 / 64:
        // total_cells = 64 bits needed
        let total_cells = arena_size / max_alignment;
        // how many u64 words to hold those bits? (64 + 63) / 64 = 1
        let bitmap_u64_len = (total_cells + 63) / 64;

        // Allocate only needed size for the bitmap
        let mut bitmap = Vec::with_capacity(bitmap_u64_len);
        bitmap.resize(bitmap_u64_len, 0);

        Ok(Self {
            bitmap,
            layout,
            buffer,
            _marker: PhantomData,
        })
    }

    pub fn alloc<T>(&mut self, value: T) -> ArenaPointer<'arena, T> {
        self.try_alloc(value).unwrap()
    }

    /// Allocate a value and return that value.
    pub fn try_alloc<T>(&mut self, value: T) -> Result<ArenaPointer<'arena, T>, ArenaAllocError> {
        let allocation_data = self.get_allocation_data::<T>()?;
        // SAFETY: We have checked that the allocation is valid.
        unsafe { Ok(self.alloc_unchecked(value, allocation_data)) }
    }

    pub unsafe fn alloc_unchecked<T>(
        &mut self,
        value: T,
        alloc_data: ArenaAllocationData,
    ) -> ArenaPointer<'arena, T> {
        // 1. Mark as allocated
        self.mark_range(alloc_data.bit_index, alloc_data.required_cells, true);

        // 2. Calculate physical address
        let offset = alloc_data.bit_index * alloc_data.required_cells;
        unsafe {
            let dst = self.buffer.as_ptr().add(offset) as *mut T;
            dst.write(value);

            ArenaPointer::from_raw(NonNull::new_unchecked(dst))
        }
    }

    pub fn get_allocation_data<T>(&self) -> Result<ArenaAllocationData, ArenaAllocError> {
        let layout = Layout::new::<T>();

        // Safety check: Ensure the object doesn't require MORE alignment
        // than our Arena cells provide (64 bytes).
        if layout.align() > self.layout.align() {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        // Snapping the size to our 64-byte grid
        let cell_size = self.layout.align(); // 64
        let required_cells = (layout.size() + (cell_size - 1)) / cell_size;

        // 2. Find space in the bitmap
        let bit_index = self
            .find_free_cells(required_cells)
            .ok_or(ArenaAllocError::OutOfMemory)?;

        Ok(ArenaAllocationData {
            bit_index,
            required_cells,
        })
    }

    /// Helper to set (allocate) or clear (deallocate) a range of cells
    pub fn mark_range(&mut self, start_bit: usize, cells: usize, is_allocated: bool) {
        for i in 0..cells {
            let bit_idx = start_bit + i;
            let word_idx = bit_idx / 64;
            let bit_pos = bit_idx % 64;

            if is_allocated {
                self.bitmap[word_idx] |= 1 << bit_pos;
            } else {
                self.bitmap[word_idx] &= !(1 << bit_pos);
            }
        }
    }

    /// Searches the bitmap for 'count' consecutive free bits (0s).
    fn find_free_cells(&self, count: usize) -> Option<usize> {
        // For a single cell request (most common in JS), this is ultra-fast.
        if count == 1 {
            for (word_idx, &word) in self.bitmap.iter().enumerate() {
                if word != !0u64 {
                    // If the word isn't full (all 1s)
                    let bit_pos = (!word).trailing_zeros() as usize;
                    return Some(word_idx * 64 + bit_pos);
                }
            }
            return None;
        }

        // Multi-cell allocation search (e.g., for large objects)
        self.find_consecutive_bits(count)
    }

    fn find_consecutive_bits(&self, count: usize) -> Option<usize> {
        let mut continuous_free = 0;
        let mut start_index = 0;

        // Total bits to check
        let total_bits = self.bitmap.len() * 64;

        for i in 0..total_bits {
            let word_idx = i / 64;
            let bit_pos = i % 64;

            let is_free = (self.bitmap[word_idx] & (1 << bit_pos)) == 0;

            if is_free {
                if continuous_free == 0 {
                    start_index = i;
                }
                continuous_free += 1;
                if continuous_free == count {
                    return Some(start_index);
                }
            } else {
                continuous_free = 0;
            }
        }
        None
    }
}
