//! This module implements an Arena allocator in Rust.

// Implementation notes:
//
// This implementation is based off the GingerBill blog post:
//
// https://www.gingerbill.org/article/2019/02/08/memory-allocation-strategies-002/

use core::{alloc::LayoutError, marker::PhantomData, ptr::NonNull};

use rust_alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};

use finalize::Finalize;

pub mod boxed;
pub mod finalize;

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

pub struct ArenaPtr<'arena, T>(NonNull<T>, PhantomData<&'arena ()>);

impl<'arena, T> ArenaPtr<'arena, T> {
    unsafe fn from_raw(raw: NonNull<T>) -> Self {
        Self(raw, PhantomData)
    }

    fn to_non_null(&self) -> NonNull<T> {
        self.0
    }
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
    pub layout: Layout,
    pub previous_offset: usize,
    pub current_offset: usize,
    pub buffer: NonNull<u8>,
    _marker: PhantomData<&'arena ()>,
}

impl<'arena> Arena<'arena> {
    pub fn try_init(arena_size: usize, alignment: usize) -> Result<Self, ArenaAllocError> {
        let layout = Layout::from_size_align(arena_size, alignment)?;
        let data = unsafe {
            let data = alloc(layout);
            let Some(data) = NonNull::new(data) else {
                handle_alloc_error(layout)
            };
            data
        };

        Ok(Self {
            layout,
            previous_offset: 0,
            current_offset: 0,
            buffer: data,
            _marker: PhantomData,
        })
    }

    pub fn alloc<T: Finalize>(&mut self, value: T) -> ArenaPtr<'arena, T> {
        self.try_alloc(value).unwrap()
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
    pub fn try_alloc<T: Finalize>(
        &mut self,
        value: T,
    ) -> Result<ArenaPtr<'arena, T>, ArenaAllocError> {
        let size = core::mem::size_of::<T>();
        let alignment = core::mem::align_of_val(&value);

        // Safety: This is safe as `current_offset` must be less then the length
        // of the buffer.
        let current = unsafe { self.buffer.add(self.current_offset) };

        // Determine the alignment offset needed to align.
        let offset = current.align_offset(alignment);

        // Check for alignment failure case
        if offset == usize::MAX {
            return Err(ArenaAllocError::AlignmentNotPossible);
        }

        let new_buffer_offset = self.current_offset + offset;

        // Check that we won't overflow the memory block
        if new_buffer_offset + size > self.layout.size() {
            return Err(ArenaAllocError::OutOfMemory);
        }

        self.previous_offset = new_buffer_offset;
        self.current_offset += offset + size;

        // Safety:
        //
        // The calculated destination is within a valid range.
        //
        // We have determined that we will not overflow the range of the Arena's buffer
        // and that the alignment is possible.
        unsafe {
            let dst = self.buffer.as_ptr().add(new_buffer_offset).cast::<T>();
            dst.write(value);
            Ok(ArenaPtr::from_raw(NonNull::new_unchecked(dst)))
        }
    }
}

impl<'arena> Drop for Arena<'arena> {
    fn drop(&mut self) {
        unsafe { dealloc(self.buffer.as_ptr(), self.layout) };
    }
}

#[cfg(test)]
mod tests {
    use crate::arena::{Arena, boxed::Box, finalize::Finalize};

    const DEFAULT_PAGE_SIZE: usize = 4096;

    fn create_arena_allocator<'arena>() -> Arena<'arena> {
        Arena::try_init(DEFAULT_PAGE_SIZE, 16).expect("A valid arena alloc initialization.")
    }

    #[test]
    fn arena_alloc_integers() {
        // 4096 byte page size
        let mut allocator = create_arena_allocator();
        for i in 0..1024 {
            let _ = allocator.alloc(i);
        }
        assert!(allocator.try_alloc(0u8).is_err());
    }

    #[test]
    fn arena_alloc_misc() {
        use rust_alloc::boxed::Box;
        use rust_alloc::collections::LinkedList;

        // 32 byte struct (24 bytes + 8 padding) -> 128 fit inside a 4096 page
        struct MiscItem {
            _one: u64,
            _two: u128,
        }

        impl Finalize for MiscItem {}

        // 4096 byte page size
        let mut allocator = create_arena_allocator();

        let mut list = LinkedList::new();
        for i in 0..63 {
            let value = MiscItem {
                _one: i,
                _two: i as u128,
            };
            let pointer = allocator.alloc(value);
            let boxed = unsafe { Box::from_raw(pointer.0.as_ptr()) };
            list.push_back(boxed);
        }

        // Add 32 bytes of integers
        for i in 0..8 {
            let _ = allocator.alloc(i);
        }

        // Add the final structs
        for i in 0..64 {
            let value = MiscItem {
                _one: i,
                _two: i as u128,
            };
            let _ = allocator.alloc(value);
        }

        for item in list {
            let _ptr = Box::into_raw(item);
        }

        // Assert we've reached the page miximum
        assert!(allocator.try_alloc(0u8).is_err());
    }

    #[test]
    fn test_arc_drop() {
        use core::sync::atomic::{AtomicBool, Ordering};
        use rust_alloc::rc::Rc;

        struct MyS {
            dropped: Rc<AtomicBool>,
        }

        impl Finalize for MyS {
            fn finalize(&self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Rc::new(AtomicBool::new(false));

        let mut arena = create_arena_allocator();
        let a = arena.alloc(MyS {
            dropped: dropped.clone(),
        });

        let boxed = Box::from_arena_ptr(a);

        // dropping a box just runs its finalizer.
        drop(boxed);

        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn test_double_free() {
        let mut arena = Arena::try_init(4, 4).expect("A valid arena alloc initialization.");
        let val = arena.alloc(0i32);

        let boxed = Box::from_arena_ptr(val);

        drop(boxed);
        drop(arena);
    }
}
