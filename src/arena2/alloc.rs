use core::{
    marker::PhantomData,
    ptr::{NonNull, drop_in_place},
};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use crate::arena2::ArenaAllocError;

#[derive(Debug)]
#[repr(C)]
pub struct ArenaHeapItem<T> {
    next: TaggedPtr<ErasedHeapItem>,
    value: T,
}

impl<T> ArenaHeapItem<T> {
    fn new(next: *mut ErasedHeapItem, value: T) -> Self {
        Self {
            next: TaggedPtr(next),
            value,
        }
    }

    fn mark_dropped(&mut self) {
        if !self.next.is_tagged() {
            self.next.tag()
        }
    }

    fn is_dropped(&self) -> bool {
        self.next.is_tagged()
    }
}

impl<T> Drop for ArenaHeapItem<T> {
    fn drop(&mut self) {
        unsafe {
            if !self.is_dropped() {
                self.mark_dropped();
                drop_in_place(&mut self.value)
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ErasedHeapItem {
    next: TaggedPtr<usize>,
    buf: NonNull<u8>, // Start of a byte buffer
}

impl ErasedHeapItem {
    pub fn get<T>(&self) -> NonNull<T> {
        self.buf.cast::<T>()
    }

    pub fn as_ref<T>(&self) -> &T {
        unsafe { self.get().as_ref() }
    }

    pub fn mark_dropped(&mut self) {
        if !self.next.is_tagged() {
            self.next.tag()
        }
    }

    pub fn is_dropped(&self) -> bool {
        self.next.is_tagged()
    }
}

const MASK: usize = 1usize << (usize::BITS as usize - 1usize);

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct TaggedPtr<T>(*mut T);

impl<T> TaggedPtr<T> {
    fn tag(&mut self) {
        self.0 = self.0.map_addr(|addr| addr | MASK);
    }

    fn is_tagged(&self) -> bool {
        self.0 as usize & MASK == MASK
    }

    fn as_ptr(&self) -> *mut T {
        self.0.map_addr(|addr| addr ^ MASK)
    }
}

// An arena pointer
//
// NOTE: This will actually need to be an offset at some point if we were to add
// serialization. That's because the underlying pointer is unreliable, so we
// would always need to derive the actual pointer from the Arena's buffer pointer

#[repr(transparent)]
pub struct ArenaPtr<'arena, T>(NonNull<ErasedHeapItem>, PhantomData<&'arena T>);

impl<'arena, T> ArenaPtr<'arena, T> {
    unsafe fn from_raw(raw: NonNull<ArenaHeapItem<T>>) -> Self {
        Self(raw.cast::<ErasedHeapItem>(), PhantomData)
    }

    pub fn as_ref(&self) -> &'arena T {
        // SAFETY: HeapItem is non-null and valid for dereferencing.
        unsafe {
            let typed_ptr = self.0.as_ptr().cast::<ArenaHeapItem<T>>();
            &(*typed_ptr).value
        }
    }
}

impl<'arena, T> Drop for ArenaPtr<'arena, T> {
    fn drop(&mut self) {
        unsafe {
            // Cast and drop inner value
            let mut typed_ptr = self.0.cast::<ArenaHeapItem<T>>();
            let inner = typed_ptr.as_mut();
            drop_in_place(&mut inner.value);
            inner.mark_dropped();
        }
    }
}

const FULL_MASK: u8 = 0b0100_0000;

#[derive(Debug, Default)]
pub struct ArenaState(u8);

impl ArenaState {
    pub fn set_full(&mut self) {
        self.0 |= FULL_MASK
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
    pub flags: ArenaState,
    pub layout: Layout,
    pub last_allocation: *mut ErasedHeapItem,
    pub current_offset: usize,
    pub buffer: NonNull<u8>,
    _marker: PhantomData<&'arena ()>,
}

impl<'arena> Arena<'arena> {
    // TODO: We need to account for minimum alignment on non x86 platforms
    pub fn try_init(arena_size: usize, max_alignment: usize) -> Result<Self, ArenaAllocError> {
        let layout = Layout::from_size_align(arena_size, max_alignment)?;
        let data = unsafe {
            let data = alloc(layout);
            let Some(data) = NonNull::new(data) else {
                handle_alloc_error(layout)
            };
            data
        };

        Ok(Self {
            flags: ArenaState::default(),
            layout,
            last_allocation: core::ptr::null_mut::<ErasedHeapItem>(), // NOTE: watch this one.
            current_offset: 0,
            buffer: data,
            _marker: PhantomData,
        })
    }

    pub fn close(&mut self) {
        self.flags.set_full();
    }

    pub fn alloc<T>(&mut self, value: T) -> ArenaPtr<'arena, T> {
        self.try_alloc(value).unwrap()
    }

    /// Allocates
    pub fn alloc_or_close<T>(
        &mut self,
        value: T,
    ) -> Result<Option<ArenaPtr<'arena, T>>, ArenaAllocError> {
        match self.try_alloc(value) {
            Ok(v) => Ok(Some(v)),
            Err(ArenaAllocError::OutOfMemory) => {
                self.flags.set_full();
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
    pub fn try_alloc<T>(&mut self, value: T) -> Result<ArenaPtr<'arena, T>, ArenaAllocError> {
        let allocation_data = self.get_allocation_data(&value)?;

        // SAFETY: We have checked that the allocation is valid.
        unsafe { Ok(self.alloc_unchecked(value, allocation_data)) }
    }

    pub unsafe fn alloc_unchecked<T>(
        &mut self,
        value: T,
        allocation_data: ArenaAllocationData,
    ) -> ArenaPtr<'arena, T> {
        unsafe {
            // Calculate required values
            self.current_offset += allocation_data.relative_offset + allocation_data.size;

            let buffer_ptr = self.buffer.as_ptr();
            let dst = buffer_ptr
                .add(allocation_data.buffer_offset)
                .cast::<ArenaHeapItem<T>>();
            // NOTE: everyI recomm next begin by pointing back to the start of the buffer rather than null.
            let arena_heap_item = ArenaHeapItem::new(self.last_allocation, value);
            dst.write(arena_heap_item);
            // We've written the last_allocation to the heap, so update with a pointer to dst
            self.last_allocation = dst as *mut ErasedHeapItem;
            ArenaPtr::from_raw(NonNull::new_unchecked(dst))
        }
    }

    pub fn get_allocation_data<T>(
        &self,
        value_ref: &T,
    ) -> Result<ArenaAllocationData, ArenaAllocError> {
        let size = core::mem::size_of::<ArenaHeapItem<T>>();
        let alignment = core::mem::align_of_val(value_ref);

        assert!(alignment <= self.layout.align());

        // Safety: This is safe as `current_offset` must be less then the length
        // of the buffer.
        let current = unsafe { self.buffer.add(self.current_offset) };

        // Determine the alignment offset needed to align.
        let relative_offset = current.align_offset(alignment);

        // Check for alignment failure case
        if relative_offset == usize::MAX {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        let buffer_offset = self.current_offset + relative_offset;

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

    /// Walks the Arena allocations to determine if the arena is droppable
    pub fn run_drop_check(&mut self) -> bool {
        let mut unchecked_ptr = self.last_allocation;
        while let Some(node) = NonNull::new(unchecked_ptr) {
            let item = unsafe { node.as_ref() };
            if !item.is_dropped() {
                return false;
            }
            unchecked_ptr = item.next.as_ptr() as *mut ErasedHeapItem
        }
        true
    }
}

impl<'arena> Drop for Arena<'arena> {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}
