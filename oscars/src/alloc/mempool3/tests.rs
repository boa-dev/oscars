use core::ptr::NonNull;
use rust_alloc::vec::Vec;

use crate::alloc::mempool3::PoolItem;

use super::PoolAllocator;

#[test]
fn alloc_dealloc() {
    // Let's just allocate with half a Kb per page
    let mut allocator = PoolAllocator::default().with_page_size(512);

    let mut first_region: Vec<NonNull<PoolItem<i32>>> = Vec::default();
    for i in 0..32 {
        let ap = allocator.try_alloc(i).unwrap();
        first_region.push(ap.as_ptr());
    }
    assert!(
        allocator.pools_len() >= 1,
        "at least one pool must exist after allocations"
    );

    let mut second_region: Vec<NonNull<PoolItem<i32>>> = Vec::default();
    for i in 0..32 {
        let ap = allocator.try_alloc(i).unwrap();
        second_region.push(ap.as_ptr());
    }
    assert!(
        allocator.pools_len() >= 1,
        "pools must still exist after further allocations"
    );

    // release first region via free_slot
    for ptr in first_region {
        allocator.free_slot(ptr.cast::<u8>());
    }
    allocator.drop_empty_pools();

    // there may or may not be one fewer pool depending on slot packing, but
    // the allocator must still contain the second region
    assert!(allocator.pools_len() <= 2);
    drop(second_region);
}

#[test]
fn free_list_reclaims_slots() {
    let mut allocator = PoolAllocator::default().with_page_size(4096);

    let mut ptrs: Vec<NonNull<PoolItem<u64>>> = (0u64..32)
        .map(|i| allocator.try_alloc(i).unwrap().as_ptr())
        .collect();

    let pools_after_alloc = allocator.pools_len();
    assert!(pools_after_alloc >= 1);

    // free the first 16 slots via free_slot
    let to_free = ptrs.drain(..16).collect::<Vec<_>>();
    for ptr in to_free {
        allocator.free_slot(ptr.cast::<u8>());
    }

    // reallocate 16 more items, they should reuse freed slots not create
    // a new pool
    for i in 32u64..48 {
        let _ = allocator.try_alloc(i).unwrap();
    }

    assert_eq!(
        allocator.pools_len(),
        pools_after_alloc,
        "free list must allow slot reuse without new pools"
    );
}

// bitmap drop check, if all slots freed then pool is reclaimed
#[test]
fn bitmap_drop_check() {
    let mut allocator = PoolAllocator::default().with_page_size(4096);

    let ptrs: Vec<NonNull<PoolItem<u64>>> = (0u64..16)
        .map(|i| allocator.try_alloc(i).unwrap().as_ptr())
        .collect();

    assert_eq!(allocator.pools_len(), 1);

    for ptr in ptrs {
        allocator.free_slot(ptr.cast::<u8>());
    }

    allocator.drop_empty_pools();

    assert_eq!(
        allocator.pools_len(),
        0,
        "all-empty pool must be dropped by drop_empty_pools"
    );
}

#[test]
fn arc_drop() {
    use core::sync::atomic::{AtomicBool, Ordering};
    use rust_alloc::rc::Rc;

    struct MyS {
        dropped: Rc<AtomicBool>,
    }

    impl Drop for MyS {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    let dropped = Rc::new(AtomicBool::new(false));

    let mut allocator = PoolAllocator::default();
    let a = allocator
        .try_alloc(MyS {
            dropped: dropped.clone(),
        })
        .unwrap();

    assert_eq!(allocator.pools_len(), 1);

    // drop the inner value and return the slot to the allocator
    let pool_item_ptr = a.as_ptr();
    unsafe {
        allocator.free_slot_typed(pool_item_ptr);
    }

    assert!(dropped.load(Ordering::SeqCst), "destructor must have run");
    assert_eq!(allocator.pools_len(), 1);

    allocator.drop_empty_pools();
    assert_eq!(allocator.pools_len(), 0, "empty pool must be reclaimed");
}

// SlotPool slot count arithmetic tests
//
// these tests confirm that the try_init calculation produces the expected
// slot count and bitmap size for different inputs

fn slot_pool_layout(slot_size: usize, total_capacity: usize) -> (usize, usize) {
    use crate::alloc::mempool3::alloc::SlotPool;
    let pool = SlotPool::try_init(slot_size, total_capacity, 8).unwrap();
    (pool.slot_count, pool.bitmap_bytes)
}

#[test]
fn slot_count_example_from_doc() {
    let (slot_count, bitmap_bytes) = slot_pool_layout(16, 512);
    assert_eq!(bitmap_bytes, 8, "8 bytes covers 32 estimated slots");
    assert_eq!(slot_count, 31);
}

#[test]
fn slot_count_needs_two_bitmap_chunks() {
    let (slot_count, bitmap_bytes) = slot_pool_layout(8, 4096);
    assert_eq!(bitmap_bytes, 64);
    assert_eq!(slot_count, 504);
}

#[test]
fn slot_count_large_slot_size() {
    let (slot_count, bitmap_bytes) = slot_pool_layout(256, 4096);
    assert_eq!(bitmap_bytes, 8);
    assert_eq!(slot_count, 15);
}

#[test]
fn slot_count_tight_capacity() {
    let (slot_count, bitmap_bytes) = slot_pool_layout(64, 512);
    assert_eq!(bitmap_bytes, 8);
    assert_eq!(slot_count, 7);
}

/// Verify that recycled empty slot pools are reused on the next `try_alloc`
/// without allocating new OS memory, the heap_size should be unchanged.
#[test]
fn recycled_pool_avoids_realloc() {
    let mut allocator = PoolAllocator::default().with_page_size(4096);

    let ptrs: Vec<_> = (0u64..16)
        .map(|i| allocator.try_alloc(i).unwrap().as_ptr())
        .collect();
    assert_eq!(allocator.slot_pools.len(), 1);
    let heap_after_first_alloc = allocator.current_heap_size;

    for ptr in ptrs {
        allocator.free_slot(ptr.cast::<u8>());
    }
    allocator.drop_empty_pools();

    assert_eq!(allocator.slot_pools.len(), 0);
    assert_eq!(allocator.recycled_pools.len(), 1);
    assert_eq!(allocator.current_heap_size, heap_after_first_alloc);

    let heap_before_second_alloc = allocator.current_heap_size;
    for i in 16u64..32 {
        let _ = allocator.try_alloc(i).unwrap();
    }

    assert_eq!(allocator.slot_pools.len(), 1);
    assert_eq!(allocator.recycled_pools.len(), 0);
    assert_eq!(allocator.current_heap_size, heap_before_second_alloc);
}

/// Verify that when more pools become empty than `max_recycled` allows,
/// the overflow is freed to the OS.
#[test]
fn max_recycled_cap_respected() {
    let mut allocator = PoolAllocator::default().with_page_size(32);
    allocator.max_recycled = 0;

    let p1 = allocator.try_alloc(1u64).unwrap().as_ptr();
    let px = allocator.try_alloc(2u64).unwrap().as_ptr();
    let py = allocator.try_alloc(3u64).unwrap().as_ptr();
    assert_eq!(allocator.slot_pools.len(), 1);

    let p2 = allocator.try_alloc(4u64).unwrap().as_ptr();
    assert_eq!(allocator.slot_pools.len(), 2);

    let heap_before = allocator.current_heap_size;

    allocator.free_slot(p1.cast::<u8>());
    allocator.free_slot(px.cast::<u8>());
    allocator.free_slot(py.cast::<u8>());
    allocator.free_slot(p2.cast::<u8>());

    allocator.max_recycled = 1;
    allocator.drop_empty_pools();

    assert_eq!(allocator.slot_pools.len(), 0);
    assert_eq!(allocator.recycled_pools.len(), 1);
    assert!(allocator.current_heap_size < heap_before);
}
