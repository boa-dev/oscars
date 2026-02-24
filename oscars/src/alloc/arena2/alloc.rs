use core::{
    cell::Cell,
    marker::PhantomData,
    ptr::{NonNull, drop_in_place},
};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use crate::alloc::arena2::ArenaAllocError;

#[derive(Debug)]
#[repr(C)]
pub struct ArenaHeapItem<T: ?Sized> {
    next: TaggedPtr<ErasedHeapItem>,
    value: T,
}

impl<T: ?Sized> ArenaHeapItem<T> {
    fn new(next: *mut ErasedHeapItem, value: T) -> Self
    where
        T: Sized,
    {
        Self {
            next: TaggedPtr(next),
            value,
        }
    }

    pub fn mark_dropped(&mut self) {
        if !self.next.is_tagged() {
            self.next.tag()
        }
    }

    pub fn is_dropped(&self) -> bool {
        self.next.is_tagged()
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn as_ptr(&mut self) -> *mut T {
        &mut self.value as *mut T
    }

    pub(crate) fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: ?Sized> Drop for ArenaHeapItem<T> {
    fn drop(&mut self) {
        unsafe {
            if !self.is_dropped() {
                self.mark_dropped();
                drop_in_place(self.value_mut())
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

    pub fn mark_dropped(&mut self) {
        if !self.next.is_tagged() {
            self.next.tag()
        }
    }

    pub fn is_dropped(&self) -> bool {
        self.next.is_tagged()
    }
}

impl<T> core::convert::AsRef<T> for ErasedHeapItem {
    fn as_ref(&self) -> &T {
        // SAFETY: TODO
        unsafe { self.get().as_ref() }
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

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ErasedArenaPointer<'arena>(NonNull<ErasedHeapItem>, PhantomData<&'arena ()>);

impl<'arena> ErasedArenaPointer<'arena> {
    fn from_raw(raw: NonNull<ErasedHeapItem>) -> Self {
        Self(raw, PhantomData)
    }

    pub fn as_non_null(&self) -> NonNull<ErasedHeapItem> {
        self.0
    }

    pub fn as_raw_ptr(&self) -> *mut ErasedHeapItem {
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
    unsafe fn from_raw(raw: NonNull<ArenaHeapItem<T>>) -> Self {
        Self(
            ErasedArenaPointer::from_raw(raw.cast::<ErasedHeapItem>()),
            PhantomData,
        )
    }

    pub fn as_inner_ref(&self) -> &'arena T {
        // SAFETY: HeapItem is non-null and valid for dereferencing.
        unsafe {
            let typed_ptr = self.0.as_raw_ptr().cast::<ArenaHeapItem<T>>();
            &(*typed_ptr).value
        }
    }

    /// Return a pointer to the inner T
    ///
    /// SAFETY:
    ///
    /// - Caller must ensure that T is not dropped
    /// - Caller must ensure that the lifetime of T does not exceed it's Arena.
    pub fn as_ptr(&self) -> NonNull<ArenaHeapItem<T>> {
        self.0.as_non_null().cast::<ArenaHeapItem<T>>()
    }

    /// Convert the current ArenaPointer into an `ErasedArenaPointer`
    pub fn to_erased(self) -> ErasedArenaPointer<'arena> {
        self.0
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
    pub last_allocation: Cell<*mut ErasedHeapItem>,
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
            last_allocation: Cell::new(core::ptr::null_mut::<ErasedHeapItem>()), // NOTE: watch this one.
            current_offset: Cell::new(0),
            buffer: data,
            _marker: PhantomData,
        })
    }

    pub fn close(&self) {
        self.flags.set(self.flags.get().full());
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
        let allocation_data = self.get_allocation_data(&value)?;
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
            // NOTE: everyI recomm next begin by pointing back to the start of the buffer rather than null.
            let arena_heap_item = ArenaHeapItem::new(self.last_allocation.get(), value);
            dst.write(arena_heap_item);
            // We've written the last_allocation to the heap, so update with a pointer to dst
            self.last_allocation.set(dst as *mut ErasedHeapItem);
            ArenaPointer::from_raw(NonNull::new_unchecked(dst))
        }
    }

    /// Bump-allocate a raw byte region matching `layout`.
    ///
    /// Unlike `try_alloc`, this does **not** wrap the allocation in an
    /// `ArenaHeapItem` and does not touch the `last_allocation` linked
    /// list. The caller is responsible for lifetime tracking.
    ///
    /// Returns a `NonNull<[u8]>` slice covering exactly `layout.size()`
    /// bytes, aligned to at least `layout.align()`.
    pub fn try_alloc_bytes(
        &self,
        layout: Layout,
    ) -> Result<NonNull<[u8]>, ArenaAllocError> {
        let size = layout.size();
        let align = layout.align();

        if align > self.layout.align() {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        // current bump pointer
        let current = unsafe { self.buffer.add(self.current_offset.get()) };

        let padding = current.align_offset(align);
        if padding == usize::MAX {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        let buffer_offset = self.current_offset.get() + padding;

        if buffer_offset + size > self.layout.size() {
            return Err(ArenaAllocError::OutOfMemory);
        }

        // advance the bump pointer past padding + payload
        self.current_offset.set(buffer_offset + size);

        let ptr = unsafe { self.buffer.as_ptr().add(buffer_offset) };
        // safety: ptr is non-null (derived from NonNull buffer) and within bounds
        let nn = unsafe { NonNull::new_unchecked(ptr) };
        Ok(NonNull::slice_from_raw_parts(nn, size))
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

    /// Walks the Arena allocations to determine if the arena is droppable
    pub fn run_drop_check(&self) -> bool {
        let mut unchecked_ptr = self.last_allocation.get();
        while let Some(node) = NonNull::new(unchecked_ptr) {
            let item = unsafe { node.as_ref() };
            if !item.is_dropped() {
                return false;
            }
            unchecked_ptr = item.next.as_ptr() as *mut ErasedHeapItem
        }
        true
    }

    // checks dropped items in this arena
    #[cfg(test)]
    pub fn item_drop_states(&self) -> rust_alloc::vec::Vec<bool> {
        let mut result = rust_alloc::vec::Vec::new();
        let mut unchecked_ptr = self.last_allocation.get();
        while let Some(node) = NonNull::new(unchecked_ptr) {
            let item = unsafe { node.as_ref() };
            result.push(item.is_dropped());
            unchecked_ptr = item.next.as_ptr() as *mut ErasedHeapItem
        }
        result
    }
}

impl<'arena> Drop for Arena<'arena> {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}
