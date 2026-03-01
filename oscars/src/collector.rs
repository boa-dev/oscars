//! `Collector` trait and `GcAllocator` handle.

use core::cell::RefCell;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use rust_alloc::alloc::Layout;

use crate::collectors::mark_sweep::MarkSweepGarbageCollector;

/// Super trait of [`Allocator`] for garbage-collected allocators.
///
/// # Safety
///
/// Allocated memory must remain valid until deallocated or collected.
pub unsafe trait Collector: Allocator {}

/// Wraps a `&RefCell<MarkSweepGarbageCollector>` to expose `&self` allocation.
pub struct GcAllocator<'gc> {
    collector: &'gc RefCell<MarkSweepGarbageCollector>,
}

impl<'gc> GcAllocator<'gc> {
    pub fn new(collector: &'gc RefCell<MarkSweepGarbageCollector>) -> Self {
        Self { collector }
    }
}

unsafe impl Allocator for GcAllocator<'_> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let mut collector = self.collector.borrow_mut();

        if !collector.allocator.is_below_threshold() {
            collector.collect();
            if !collector.allocator.is_below_threshold() {
                collector.allocator.increase_threshold();
                collector
                    .allocator
                    .initialize_new_arena()
                    .map_err(|_| AllocError)?;
            }
        }

        if collector.allocator.arenas_len() == 0 {
            collector
                .allocator
                .initialize_new_arena()
                .map_err(|_| AllocError)?;
        }

        let ptr = if layout.size() == 0 {
            NonNull::dangling().as_ptr()
        } else {
            let raw = unsafe { rust_alloc::alloc::alloc(layout) };
            if raw.is_null() {
                return Err(AllocError);
            }
            raw
        };

        Ok(NonNull::slice_from_raw_parts(
            unsafe { NonNull::new_unchecked(ptr) },
            layout.size(),
        ))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            unsafe {
                rust_alloc::alloc::dealloc(ptr.as_ptr(), layout);
            }
        }
    }
}

unsafe impl Collector for GcAllocator<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use allocator_api2::vec::Vec as ApiVec;

    #[test]
    fn gc_allocator_basic() {
        let collector = RefCell::new(MarkSweepGarbageCollector::default());
        let alloc = GcAllocator::new(&collector);

        let mut v: ApiVec<u64, &GcAllocator> = ApiVec::new_in(&alloc);
        for i in 0..100 {
            v.push(i);
        }

        assert_eq!(v.len(), 100);
        assert_eq!(v[0], 0);
        assert_eq!(v[99], 99);
    }

    #[test]
    fn gc_allocator_zst() {
        let collector = RefCell::new(MarkSweepGarbageCollector::default());
        let alloc = GcAllocator::new(&collector);

        let mut v: ApiVec<(), &GcAllocator> = ApiVec::new_in(&alloc);
        for _ in 0..10 {
            v.push(());
        }
        assert_eq!(v.len(), 10);
    }

    #[test]
    fn gc_allocator_drop() {
        let collector = RefCell::new(MarkSweepGarbageCollector::default());
        let alloc = GcAllocator::new(&collector);

        {
            let mut v: ApiVec<u32, &GcAllocator> = ApiVec::new_in(&alloc);
            v.push(42);
            v.push(99);
        }
    }

    #[test]
    fn gc_allocator_is_collector() {
        fn assert_collector<T: Collector>(_: &T) {}

        let collector = RefCell::new(MarkSweepGarbageCollector::default());
        let alloc = GcAllocator::new(&collector);
        assert_collector(&alloc);
    }

    #[test]
    fn gc_allocator_with_strings() {
        let collector = RefCell::new(MarkSweepGarbageCollector::default());
        let alloc = GcAllocator::new(&collector);

        let mut v: ApiVec<rust_alloc::string::String, &GcAllocator> = ApiVec::new_in(&alloc);
        v.push(rust_alloc::string::String::from("hello"));
        v.push(rust_alloc::string::String::from("world"));

        assert_eq!(v[0], "hello");
        assert_eq!(v[1], "world");
    }
}
