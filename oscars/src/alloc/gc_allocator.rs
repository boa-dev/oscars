// `GcAllocator<'gc>` lets types like `Vec` allocate directly from the 
// gc's memory arena. It borrows the collector's allocator and tracks 
// active memory sizes in a map
//
// the limitations we need to fix:
// - the mark phase ignores this memory. wrapper types must implement `Trace`
// - `deallocate` is a no-op because bump allocators only free memory on drop.
// - if you put a `Gc<T>` in this vec, the gc won't trace the inner pointers.
// - arena allocations cap at 16-byte alignment
//
// after weak-map integration:
//
// when weak maps is integrated, the gc will get a registration queue. we can register
// this allocator there. that lets the gc clean up dead entries, alert 
// observers on free and actually trace the elements inside gc backed vecs

use core::cell::RefCell;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator, Layout};
use hashbrown::HashMap;

use crate::alloc::arena2::ArenaAllocator;

const MAX_ARENA_ALIGN: usize = 16;

/// [`Allocator`] compatible handle tied to the gc's arena
///
/// get one via [`Collector::allocator`], don't build it manually
///
/// the `'gc` lifetime ties this directly to the collector so it can't outlive it
///
/// limitations:
/// - single-threaded only so, `RefCell` panics on aliasing
/// - 16-byte alignment ceiling
/// - memory is only freed to the OS when the collector drops
///
/// [`Collector::allocator`]: crate::collectors::collector::Collector::allocator
pub struct GcAllocator<'gc> {
    // shared borrow of the collector's arena.
    // the arena itself is `'static` inside the collector.
    inner: &'gc RefCell<ArenaAllocator<'static>>,
    
    // (ptr address -> size). tracks live allocations.

    // TODO: let the gc clean this up once weak maps are ready
    records: RefCell<HashMap<usize, usize>>,
}

impl<'gc> GcAllocator<'gc> {
    /// Construct a `GcAllocator` that allocates into `arena`.
    ///
    /// Called by [`MarkSweepGarbageCollector::allocator()`]; prefer that
    /// method over calling this directly
    ///
    /// [`MarkSweepGarbageCollector::allocator()`]:
    ///     crate::collectors::mark_sweep::MarkSweepGarbageCollector::allocator
    pub fn from_arena(arena: &'gc RefCell<ArenaAllocator<'static>>) -> Self {
        Self {
            inner: arena,
            records: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) fn outstanding_allocs(&self) -> usize {
        self.records.borrow().len()
    }

    /// total bytes currently tracked across all live allocations
    ///
    /// this is O(n) over live allocations, meant for debug
    pub fn total_allocated_bytes(&self) -> usize {
        self.records.borrow().values().sum()
    }
}

// SAFETY: `Allocator` needs us to return valid and aligned pointers. 
// `ArenaAllocator::try_alloc_bytes` handles both
// `RefCell` stops us from aliasing mutably at runtime, which is 
// fine here because `GcAllocator` is only meant for one thread
unsafe impl<'gc> Allocator for GcAllocator<'gc> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // zsts: return a dangling but aligned pointer without touching the arena
        // SAFETY: `layout.align()` is always >= 1 for any valid `Layout`, so the
        // value is non-zero and `new_unchecked` is sound
        if layout.size() == 0 {
            let dangling = unsafe { NonNull::new_unchecked(layout.align() as *mut u8) };
            return Ok(NonNull::slice_from_raw_parts(dangling, 0));
        }

        if layout.align() > MAX_ARENA_ALIGN {
            return Err(AllocError);
        }

        // borrow the arena, allocate and drop the borrow before touching `records`
        // so the two RefCells never overlap and panic
        let block = self
            .inner
            .borrow_mut()
            .try_alloc_bytes(layout)
            .map_err(|_| AllocError)?;

        let addr = block.as_ptr() as *const u8 as usize;
        self.records.borrow_mut().insert(addr, layout.size());

        // TODO: if this allocator is registered with the gc's weak_maps queue, 
        // notify it that a new raw allocation is live so the sweep phase can see it

        Ok(block)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let block = self.allocate(layout)?;
        // zero the uninitialized arena page, skipping zsts since `allocate` returns dangling pointers for them
        if layout.size() > 0 {
            // SAFETY: `allocate` succeeded and `block` points to valid memory
            unsafe {
                core::ptr::write_bytes(block.as_ptr() as *mut u8, 0, layout.size());
            }
        }
        Ok(block)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // zsts were never recorded
        if layout.size() == 0 {
            return;
        }

        let key = ptr.as_ptr() as usize;
        if self.records.borrow_mut().remove(&key).is_none() {
            debug_assert!(
                false,
                "deallocate called with unknown pointer {ptr:p}"
            );
        }

        // note: we don't call `mark_dropped` or touch the arena.
        // `try_alloc_bytes` bypasses the linked list, so there is no node to walk.
        // the arena page is reclaimed only when the collector is dropped
        //
        // TODO: when weak maps are implemented, tell registered observers to drop their 
        // entries early instead of waiting for a sweep
    }

    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(
            new_layout.size() >= old_layout.size(),
            "grow called with smaller new_layout"
        );

        let new_block = self.allocate(new_layout)?;

        // SAFETY: both pointers are valid, non-overlapping, and `old_layout.size()`
        // bytes are readable from `ptr` and writable to `new_block`
        if old_layout.size() > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    ptr.as_ptr(),
                    new_block.as_ptr() as *mut u8,
                    old_layout.size(),
                );
            }
        }
        // SAFETY: `ptr` was allocated by this allocator with `old_layout`.
        unsafe { self.deallocate(ptr, old_layout) };

        Ok(new_block)
    }

    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: `grow()` already checks all the allocator rules for us
        let new_block = unsafe { self.grow(ptr, old_layout, new_layout)? };

        // SAFETY: the tail region is valid, exclusively writable memory within `new_block`.
        let tail = new_layout.size() - old_layout.size();
        if tail > 0 {
            unsafe {
                let tail_ptr = (new_block.as_ptr() as *mut u8).add(old_layout.size());
                core::ptr::write_bytes(tail_ptr, 0, tail);
            }
        }

        Ok(new_block)
    }

    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        debug_assert!(
            new_layout.size() <= old_layout.size(),
            "shrink called with larger new_layout"
        );

        if new_layout.size() == 0 {
            // SAFETY: `ptr` was allocated by this allocator with `old_layout`
            unsafe { self.deallocate(ptr, old_layout) };
            // SAFETY: `new_layout.align()` >= 1 for any valid `Layout`
            let dangling = unsafe { NonNull::new_unchecked(new_layout.align() as *mut u8) };
            return Ok(NonNull::slice_from_raw_parts(dangling, 0));
        }

        let new_block = self.allocate(new_layout)?;

        // SAFETY: both pointers are valid and `new_layout.size()` <=
        // `old_layout.size()`, we copy only what the new block can hold
        unsafe {
            core::ptr::copy_nonoverlapping(
                ptr.as_ptr(),
                new_block.as_ptr() as *mut u8,
                new_layout.size(),
            );
            self.deallocate(ptr, old_layout);
        }

        Ok(new_block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allocator_api2::vec::Vec as GcVec;
    use crate::collectors::mark_sweep::MarkSweepGarbageCollector;

    /// create a collector and borrow its allocator for the test closure.
    fn with_collector<F>(f: F)
    where
        F: for<'gc> FnOnce(GcAllocator<'gc>),
    {
        let collector = MarkSweepGarbageCollector::default();
        // construct the allocator handle directly from the arena.
        // `Collector` no longer carries a factory method, the collector itself
        //is now the allocator via the `Collector: Allocator` supertrait
        let alloc = GcAllocator::from_arena(&collector.allocator);
        f(alloc);
    }

    #[test]
    fn basic_alloc_and_dealloc() {
        with_collector(|alloc| {
            let layout = Layout::from_size_align(16, 8).unwrap();
            let block = alloc.allocate(layout).expect("allocation should succeed");
            assert_eq!(block.len(), 16);
            assert_eq!(alloc.outstanding_allocs(), 1);
            unsafe { alloc.deallocate(block.cast(), layout) };
            assert_eq!(alloc.outstanding_allocs(), 0);
        });
    }

    #[test]
    fn gc_backed_vec_uses_collector_arena() {
        // Allocations land in the same arena as Gc<T> objects.
        let collector = MarkSweepGarbageCollector::default();
        // Build the handle directly — the trait no longer has a factory method.
        let alloc = GcAllocator::from_arena(&collector.allocator);
        let mut v: GcVec<u64, &GcAllocator<'_>> = GcVec::new_in(&alloc);
        for i in 0..10u64 {
            v.push(i);
        }
        assert_eq!(v.len(), 10);
        assert_eq!(v[9], 9);
        // The same arena as the collector holds the data.
        assert!(collector.allocator.borrow().arenas_len() > 0);
    }

    #[test]
    fn gc_backed_vec_dealloc_on_drop() {
        with_collector(|alloc| {
            {
                let mut v: GcVec<u64, &GcAllocator<'_>> = GcVec::new_in(&alloc);
                v.push(42u64);
                assert!(alloc.outstanding_allocs() > 0);
            }
            assert_eq!(
                alloc.outstanding_allocs(),
                0,
                "expected no dangling records after vec drop"
            );
        });
    }

    #[test]
    fn zst_allocation() {
        with_collector(|alloc| {
            let layout = Layout::new::<()>();
            let block = alloc.allocate(layout).expect("zst alloc should succeed");
            assert_eq!(block.len(), 0);
            assert_eq!(alloc.outstanding_allocs(), 0);
            unsafe { alloc.deallocate(block.cast(), layout) };
        });
    }

    #[test]
    fn alignment_within_arena_limit() {
        with_collector(|alloc| {
            for align_shift in 0..=4 {
                let align = 1usize << align_shift; // 1, 2, 4, 8, 16
                let layout = Layout::from_size_align(32, align).unwrap();
                let block = alloc.allocate(layout).expect("alloc should succeed");
                let addr = block.as_ptr() as *const u8 as usize;
                assert_eq!(addr % align, 0, "not aligned to {align}");
                unsafe { alloc.deallocate(block.cast(), layout) };
            }
        });
    }

    #[test]
    fn alignment_exceeding_arena_limit_returns_error() {
        with_collector(|alloc| {
            let layout = Layout::from_size_align(64, 32).unwrap();
            assert!(alloc.allocate(layout).is_err(), "should reject align > 16");
        });
    }

    #[test]
    fn multi_alloc_interleaved_dealloc() {
        with_collector(|alloc| {
            let layout_a = Layout::from_size_align(8, 8).unwrap();
            let layout_b = Layout::from_size_align(16, 8).unwrap();
            let layout_c = Layout::from_size_align(4, 4).unwrap();

            let a = alloc.allocate(layout_a).unwrap();
            let b = alloc.allocate(layout_b).unwrap();
            let c = alloc.allocate(layout_c).unwrap();
            assert_eq!(alloc.outstanding_allocs(), 3);

            unsafe { alloc.deallocate(b.cast(), layout_b) };
            assert_eq!(alloc.outstanding_allocs(), 2);
            unsafe { alloc.deallocate(a.cast(), layout_a) };
            assert_eq!(alloc.outstanding_allocs(), 1);
            unsafe { alloc.deallocate(c.cast(), layout_c) };
            assert_eq!(alloc.outstanding_allocs(), 0);
        });
    }

    #[test]
    fn allocate_zeroed_returns_zeros() {
        with_collector(|alloc| {
            let layout = Layout::from_size_align(128, 8).unwrap();
            let block = alloc
                .allocate_zeroed(layout)
                .expect("zeroed alloc should succeed");
            let slice =
                unsafe { core::slice::from_raw_parts(block.as_ptr() as *const u8, 128) };
            assert!(slice.iter().all(|&b| b == 0), "expected all-zero bytes");
            unsafe { alloc.deallocate(block.cast(), layout) };
        });
    }

    #[test]
    fn grow_reallocation_preserves_data() {
        with_collector(|alloc| {
            let old_layout = Layout::from_size_align(16, 8).unwrap();
            let block = alloc.allocate(old_layout).unwrap();

            unsafe {
                let p = block.as_ptr() as *mut u8;
                for i in 0..16u8 {
                    p.add(i as usize).write(i + 1);
                }
            }

            let new_layout = Layout::from_size_align(64, 8).unwrap();
            let grown = unsafe { alloc.grow(block.cast(), old_layout, new_layout) }
                .expect("grow should succeed");
            assert!(grown.len() >= 64);

            let slice =
                unsafe { core::slice::from_raw_parts(grown.as_ptr() as *const u8, 16) };
            for (i, &b) in slice.iter().enumerate() {
                assert_eq!(b, (i + 1) as u8, "data mismatch at byte {i}");
            }

            unsafe { alloc.deallocate(grown.cast(), new_layout) };
        });
    }

    #[test]
    fn shrink_reallocation_preserves_prefix() {
        with_collector(|alloc| {
            let old_layout = Layout::from_size_align(64, 8).unwrap();
            let block = alloc.allocate(old_layout).unwrap();

            unsafe {
                let p = block.as_ptr() as *mut u8;
                for i in 0..16u8 {
                    p.add(i as usize).write(0xAA + i);
                }
            }

            let new_layout = Layout::from_size_align(16, 8).unwrap();
            let shrunk = unsafe { alloc.shrink(block.cast(), old_layout, new_layout) }
                .expect("shrink should succeed");
            assert!(shrunk.len() >= 16);

            let slice =
                unsafe { core::slice::from_raw_parts(shrunk.as_ptr() as *const u8, 16) };
            for (i, &b) in slice.iter().enumerate() {
                assert_eq!(b, 0xAA + i as u8, "data mismatch at byte {i}");
            }

            unsafe { alloc.deallocate(shrunk.cast(), new_layout) };
        });
    }

    #[test]
    fn total_allocated_bytes_tracking() {
        with_collector(|alloc| {
            assert_eq!(alloc.total_allocated_bytes(), 0);

            let layout_a = Layout::from_size_align(32, 8).unwrap();
            let a = alloc.allocate(layout_a).unwrap();
            assert_eq!(alloc.total_allocated_bytes(), 32);

            let layout_b = Layout::from_size_align(64, 8).unwrap();
            let b = alloc.allocate(layout_b).unwrap();
            assert_eq!(alloc.total_allocated_bytes(), 96);

            unsafe { alloc.deallocate(a.cast(), layout_a) };
            assert_eq!(alloc.total_allocated_bytes(), 64);

            unsafe { alloc.deallocate(b.cast(), layout_b) };
            assert_eq!(alloc.total_allocated_bytes(), 0);
        });
    }

    #[test]
    fn large_vec_triggers_grow() {
        with_collector(|alloc| {
            let mut v: GcVec<u64, &GcAllocator<'_>> = GcVec::new_in(&alloc);
            for i in 0..256u64 {
                v.push(i);
            }
            assert_eq!(v.len(), 256);
            for i in 0..256u64 {
                assert_eq!(v[i as usize], i, "element mismatch at index {i}");
            }
        });
    }
}
