use core::{cell::Cell, marker::PhantomData, ptr::NonNull};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use crate::alloc::arena2::ArenaAllocError;

/// Transparent wrapper for a GC value.
/// Drop state is tracked by the GC header and arena counters.
#[derive(Debug)]
#[repr(transparent)]
pub struct ArenaHeapItem<T: ?Sized>(pub T);

impl<T: ?Sized> ArenaHeapItem<T> {
    fn new(value: T) -> Self
    where
        T: Sized,
    {
        Self(value)
    }

    pub fn value(&self) -> &T {
        &self.0
    }

    pub fn as_ptr(&mut self) -> *mut T {
        &mut self.0 as *mut T
    }

    /// Returns a raw mutable pointer to the value
    ///
    /// This avoids creating a `&mut self` reference, which can lead to stacked borrows
    /// if shared references to the heap item exist
    pub(crate) fn as_value_ptr(ptr: NonNull<Self>) -> *mut T {
        // With repr(transparent), the outer struct has the same address as the inner value
        ptr.as_ptr() as *mut T
    }
}

/// Type erased pointer for arena allocations.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ErasedHeapItem(NonNull<u8>);

impl ErasedHeapItem {
    pub fn get<T>(&self) -> NonNull<T> {
        self.0.cast::<T>()
    }
}

impl<T> core::convert::AsRef<T> for ErasedHeapItem {
    fn as_ref(&self) -> &T {
        // SAFETY: caller ensures this pointer was allocated as T
        unsafe { self.get().as_ref() }
    }
}

// An arena pointer
//
// NOTE: This will actually need to be an offset at some point if we were to add
// serialization. That's because the underlying pointer is unreliable, so we
// would always need to derive the actual pointer from the Arena's buffer pointer

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ErasedArenaPointer<'arena>(NonNull<u8>, PhantomData<&'arena ()>);

impl<'arena> ErasedArenaPointer<'arena> {
    fn from_raw(raw: NonNull<u8>) -> Self {
        Self(raw, PhantomData)
    }

    pub fn as_non_null(&self) -> NonNull<ErasedHeapItem> {
        // Keep the old erased pointer API
        ErasedHeapItem(self.0).get()
    }

    pub fn as_raw_ptr(&self) -> *mut u8 {
        self.0.as_ptr()
    }

    /// Extend the lifetime of this erased arena pointer to 'static
    ///
    /// SAFETY:
    ///
    /// safe because the gc collector owns the arena and keeps it alive
    pub(crate) unsafe fn extend_lifetime(self) -> ErasedArenaPointer<'static> {
        ErasedArenaPointer(self.0, PhantomData)
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
    unsafe fn from_raw(raw: NonNull<ArenaHeapItem<T>>) -> Self {
        Self(ErasedArenaPointer::from_raw(raw.cast::<u8>()), PhantomData)
    }

    pub fn as_inner_ref(&self) -> &'arena T {
        // SAFETY: pointer is valid, ArenaHeapItem<T> is repr(transparent) over T.
        unsafe {
            let typed_ptr = self.0.as_raw_ptr().cast::<ArenaHeapItem<T>>();
            &(*typed_ptr).0
        }
    }

    /// Return a pointer to the inner T
    ///
    /// SAFETY:
    ///
    /// - Caller must ensure that T is not dropped
    /// - Caller must ensure that the lifetime of T does not exceed it's Arena.
    pub fn as_ptr(&self) -> NonNull<ArenaHeapItem<T>> {
        self.0.0.cast::<ArenaHeapItem<T>>()
    }

    /// Convert the current ArenaPointer into an `ErasedArenaPointer`
    pub fn to_erased(self) -> ErasedArenaPointer<'arena> {
        self.0
    }

    /// Extend the lifetime of this arena pointer to 'static
    ///
    /// SAFETY:
    ///
    /// safe because the gc collector owns the arena and keeps it alive
    pub(crate) unsafe fn extend_lifetime(self) -> ArenaPointer<'static, T> {
        // SAFETY: upheld by caller
        ArenaPointer(unsafe { self.0.extend_lifetime() }, PhantomData)
    }
}

const FULL_MASK: u8 = 0b0100_0000;

#[derive(Debug, Default, Clone, Copy)]
pub struct ArenaState(u8);

impl ArenaState {
    pub fn full(&self) -> Self {
        Self(self.0 | FULL_MASK)
    }

    pub fn is_full(&self) -> bool {
        self.0 & FULL_MASK == FULL_MASK
    }
}

pub struct ArenaAllocationData {
    size: usize,
    buffer_offset: usize,
    relative_offset: usize,
}

/// An `ArenaAllocator` written in Rust.
///
/// This allocator takes advantage of the global Rust allocator to allow
/// allocating objects into a contiguous block of memory, regardless of size
/// or alignment.
///
/// The benefits of an arena allocator is to take advantage of minimal heap
/// fragmentation.
#[derive(Debug)]
#[repr(C)]
pub struct Arena<'arena> {
    pub flags: Cell<ArenaState>,
    pub layout: Layout,
    /// Number of allocations made in this arena
    alloc_count: Cell<usize>,
    /// Number of items marked as dropped
    drop_count: Cell<usize>,
    pub current_offset: Cell<usize>,
    pub buffer: NonNull<u8>,
    _marker: PhantomData<&'arena ()>,
}

impl<'arena> Arena<'arena> {
    // TODO: We need to account for minimum alignment on non x86 platforms
    pub fn try_init(
        arena_size: usize,
        max_alignment: usize,
    ) -> Result<Arena<'arena>, ArenaAllocError> {
        let layout = Layout::from_size_align(arena_size, max_alignment)?;
        let data = unsafe {
            let data = alloc(layout);
            let Some(data) = NonNull::new(data) else {
                handle_alloc_error(layout)
            };
            data
        };

        Ok(Self {
            flags: Cell::new(ArenaState::default()),
            layout,
            alloc_count: Cell::new(0),
            drop_count: Cell::new(0),
            current_offset: Cell::new(0),
            buffer: data,
            _marker: PhantomData,
        })
    }

    pub fn close(&self) {
        self.flags.set(self.flags.get().full());
    }

    /// Increment the drop counter.
    pub fn mark_dropped(&self) {
        self.drop_count.set(self.drop_count.get() + 1);
    }

    pub fn alloc<T>(&self, value: T) -> ArenaPointer<'arena, T> {
        self.try_alloc(value).unwrap()
    }

    /// Allocates
    pub fn alloc_or_close<T>(
        &self,
        value: T,
    ) -> Result<Option<ArenaPointer<'arena, T>>, ArenaAllocError> {
        let allocation = self.try_alloc(value);
        match allocation {
            Ok(v) => Ok(Some(v)),
            Err(ArenaAllocError::OutOfMemory) => {
                self.flags.set(self.flags.get().full());
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    // HUGE TODO: I think this is probably wildly unsafe, if the returned NonNull<T> is ever
    // dropped while we still own then memory, then we may run into a double free
    // situation.
    //
    // A quick solution may be to return our own NonNull pointer type, or our own Box
    // type that points to the NonNull memory.
    //
    // Or maybe `try_alloc` and `alloc` should just be considered unsafe.

    /// Allocate a value and return that value.
    pub fn try_alloc<T>(&self, value: T) -> Result<ArenaPointer<'arena, T>, ArenaAllocError> {
        let allocation_data = self.get_allocation_data::<T>()?;
        // SAFETY: We have checked that the allocation is valid.
        unsafe { Ok(self.alloc_unchecked(value, allocation_data)) }
    }

    pub unsafe fn alloc_unchecked<T>(
        &self,
        value: T,
        allocation_data: ArenaAllocationData,
    ) -> ArenaPointer<'arena, T> {
        unsafe {
            // Calculate required values
            let new_current_offset =
                self.current_offset.get() + allocation_data.relative_offset + allocation_data.size;
            self.current_offset.set(new_current_offset);

            let buffer_ptr = self.buffer.as_ptr();
            let dst = buffer_ptr
                .add(allocation_data.buffer_offset)
                .cast::<ArenaHeapItem<T>>();
            // Write the value
            let arena_heap_item = ArenaHeapItem::new(value);
            dst.write(arena_heap_item);
            // Track live/drop state with counters.
            self.alloc_count.set(self.alloc_count.get() + 1);
            ArenaPointer::from_raw(NonNull::new_unchecked(dst))
        }
    }

    pub fn get_allocation_data<T>(&self) -> Result<ArenaAllocationData, ArenaAllocError> {
        let size = core::mem::size_of::<ArenaHeapItem<T>>();
        let alignment = core::mem::align_of::<ArenaHeapItem<T>>();

        // The arena's buffer must be at least as aligned as the value we are storing.
        if alignment > self.layout.align() {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        // Safety: This is safe as `current_offset` must be less then the length
        // of the buffer.
        let current = unsafe { self.buffer.add(self.current_offset.get()) };

        // Determine the alignment offset needed to align.
        let relative_offset = current.align_offset(alignment);

        // Check for alignment failure case
        if relative_offset == usize::MAX {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        let buffer_offset = self.current_offset.get() + relative_offset;

        // Check that we won't overflow the memory block
        if buffer_offset + size > self.layout.size() {
            return Err(ArenaAllocError::OutOfMemory);
        }

        Ok(ArenaAllocationData {
            size,
            buffer_offset,
            relative_offset,
        })
    }

    /// Returns true when all allocations were marked dropped.
    pub fn run_drop_check(&self) -> bool {
        self.alloc_count.get() == self.drop_count.get()
    }

    /// Reset arena to its initial empty state, reusing the existing OS buffer.
    /// Must only be called when `run_drop_check()` is true (all items dropped).
    pub fn reset(&self) {
        debug_assert!(
            self.run_drop_check(),
            "reset() called on an arena with live items"
        );
        // Zero the buffer so stale object graphs are not observable after recycling.
        // SAFETY: buffer is valid for the full layout size and was allocated with
        // the same layout in try_init.
        unsafe { core::ptr::write_bytes(self.buffer.as_ptr(), 0, self.layout.size()) };
        self.flags.set(ArenaState::default());
        self.alloc_count.set(0);
        self.drop_count.set(0);
        self.current_offset.set(0);
    }
}

impl<'arena> Drop for Arena<'arena> {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}
