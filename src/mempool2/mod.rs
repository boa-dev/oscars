//! A block based mempool

use alloc::alloc::{LayoutError, alloc, handle_alloc_error, dealloc};
use core::alloc::Layout;
use core::ptr::{drop_in_place, NonNull};
use core::ptr;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub enum PoolAllocError {
    LayoutError(LayoutError),
    OutOfMemory,
    OutOfChunks,
}

impl From<LayoutError> for PoolAllocError {
    fn from(value: LayoutError) -> Self {
        Self::LayoutError(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NextOrNull(*mut FreeChunk);

impl NextOrNull {
    // Yeah, this is dangerous, track this.
    const fn null() -> Self {
        NextOrNull(ptr::null::<*const FreeChunk>() as *mut FreeChunk)
    }

    fn from_raw(raw: *mut FreeChunk) -> Self {
        Self(raw)
    }

    fn get(self) -> *mut FreeChunk {
        self.0
    }
}

#[derive(Debug)]
#[repr(C)]
struct FreeChunk {
    next: NextOrNull,
}

impl FreeChunk {
    const fn empty() -> Self {
        Self { next: NextOrNull::null() }
    }
}

/// A pool allocator
///
/// The pool allocator allocates a chunk of memory and subdivides it into specific sizes.
///
#[repr(C)]
pub struct Pool {
    layout: Layout,
    chunk_size: usize,
    free_head: *mut FreeChunk,
    data: NonNull<u8>,
}


impl Pool {
    pub fn init(chunk_size: usize, page_size: usize, align: usize) -> Result<Self, PoolAllocError> {
        // Create the layout for the page, align it to the chunk alignment.
        let layout = Layout::from_size_align(page_size, align)?;

        // Allocate the data memory
        //
        // SAFETY: We ensure that the returned allocated memory is not null.
        let data = unsafe {
            let data = alloc(layout);
            let Some(data) = NonNull::new(data) else {
                handle_alloc_error(layout)
            };
            data
        };

        // We need to compute aligned chunk size.
        let aligned_chunk_size = aligned_chunk_size(chunk_size, align);

        assert!(aligned_chunk_size <= page_size);

        let mut pool = Pool {
            layout,
            chunk_size: aligned_chunk_size,
            free_head: ptr::null::<*const FreeChunk>() as *mut FreeChunk, // Note the tail node.
            data,
        };

        pool.free_all();

        Ok(pool)
    }

    /// Allocate a value to this pool.
    pub fn alloc<T>(&mut self, value: T) -> NonNull<T> {
        self.try_alloc(value).expect("out of chunks to allocate")
    }

    /// Try to allocate a value to this pool
    pub fn try_alloc<T>(&mut self, value: T) -> Result<NonNull<T>, PoolAllocError> {
        let next = NonNull::new(self.free_head);
        let Some(chunk) = next else {
            return Err(PoolAllocError::OutOfChunks);
        };

        // Assert that T is equal to size or less than chunk size.
        assert!(size_of::<T>() <= self.chunk_size);

        // Assert we are not allocating a ZST
        assert_ne!(size_of::<T>(), 0);

        // Pop the chunk from the free list
        //
        // SAFETY: Chunk is safe to dereference. It is well aligned by design of
        // the allocator, and a valid value of type `FreeChunk`
        unsafe {
            self.free_head =  chunk.as_ref().next.get();
        }

        let dst = chunk.cast::<T>();

        // SAFETY: The `dst` has been popped from the free list and is a valid
        // destination to be written.
        unsafe { dst.write(value); }

        Ok(dst)
    }

    // deallocate the chunk and move it back to the free list.
    pub unsafe fn dealloc<T: Drop>(&mut self, ptr: NonNull<T>) {
        // Check that the pointer is within the bounds of the owned data block.
        assert!(self.data.as_ptr() as usize <= ptr.as_ptr() as usize && self.data.as_ptr() as usize + self.layout.size() - self.chunk_size >= ptr.as_ptr() as usize);

        // SAFETY: TODO
        unsafe {
            let ptr = ptr.as_ptr();
            drop_in_place(ptr);

            // Write an empty free chunk to the pointer.
            let dst = ptr.cast::<FreeChunk>();
            dst.write(FreeChunk::empty());
            // NOTE: We handle any potential `null` derefence here with `NextOrNull`
            (*dst).next = NextOrNull::from_raw(self.free_head);
            self.free_head = dst;
        };
    }

    fn free_all(&mut self) {
        let chunk_count = self.layout.size() / self.chunk_size;

        for i in 0..chunk_count {
            let chunk_offset = i * self.chunk_size;
            // Check that we are in the bounds of the page size.
            assert!(chunk_offset + self.chunk_size <= self.layout.size());
            // We add the offset to our data pointer and cast it to a chunk.
            //
            // SAFETY: todo
            unsafe {
                let chunk_ptr = self.data.as_ptr().add(chunk_offset) as *mut FreeChunk;
                // Push the Chunk onto the free list.
                (*chunk_ptr).next = NextOrNull::from_raw(self.free_head);
                self.free_head = chunk_ptr;
            }
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { dealloc(self.data.as_ptr(), self.layout) }
    }
}

fn is_power_of_two(x: usize) -> bool {
    (x & (x-1)) == 0
}

fn aligned_chunk_size(size: usize, align: usize) -> usize {
    let mut aligned_size = size;

	assert!(is_power_of_two(align));

	let modulo = size % align;
	if modulo != 0 {
		aligned_size += align - modulo;
	}
	return aligned_size;
}

