use core::ptr::{NonNull, drop_in_place};

use rust_alloc::vec::Vec;

use crate::alloc::arena2::ArenaHeapItem;

use super::ArenaAllocator;

#[test]
fn alloc_dealloc() {
    // Ensure the arena holds exactly `BATCH` `ArenaHeapItem<i32>` values.
    //
    // Note: we calculate this with `size_of` because ArenaHeapItem<i32> is
    // smaller on 32-bit targets than on 64-bit targets, so a fixed byte size
    // would not test the same item count on both.
    const BATCH: usize = 32;
    const ARENA_SIZE: usize = BATCH * core::mem::size_of::<ArenaHeapItem<i32>>();

    let mut allocator = ArenaAllocator::default().with_arena_size(ARENA_SIZE);

    let mut first_region = Vec::default();
    for i in 0..32_i32 {
        let value = allocator.try_alloc(i).unwrap();
        first_region.push(value.as_ptr());
    }
    assert_eq!(allocator.arenas_len(), 1);

    let mut second_region = Vec::default();
    for i in 0..32_i32 {
        let value = allocator.try_alloc(i).unwrap();
        second_region.push(value.as_ptr());
    }
    assert_eq!(allocator.arenas_len(), 2);

    // Drop all the items in the first region
    manual_drop(&mut allocator, first_region);

    // Drop dead pages, only the first arena is fully dropped, the second
    // arena remains live because none of its items have been marked dropped.
    allocator.drop_dead_arenas();

    assert_eq!(allocator.arenas_len(), 1);
}

fn manual_drop(allocator: &mut ArenaAllocator<'_>, region: Vec<NonNull<ArenaHeapItem<i32>>>) {
    for item in region {
        unsafe {
            allocator.mark_dropped(item.as_ptr() as *const u8);
        }
    }
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

    let mut allocator = ArenaAllocator::default();
    let a = allocator
        .try_alloc(MyS {
            dropped: dropped.clone(),
        })
        .unwrap();

    assert_eq!(allocator.arenas_len(), 1);

    // dropping a box just runs its finalizer.
    let heap_item = a.as_ptr();
    unsafe {
        // Manually drop the heap item
        drop_in_place(ArenaHeapItem::as_value_ptr(heap_item));
        allocator.mark_dropped(heap_item.as_ptr() as *const u8);
    };

    assert!(dropped.load(Ordering::SeqCst));

    assert_eq!(allocator.arenas_len(), 1);

    allocator.drop_dead_arenas();

    assert_eq!(allocator.arenas_len(), 0);
}

#[test]
fn recycled_arena_avoids_realloc() {
    let mut allocator = ArenaAllocator::default().with_arena_size(512);

    let mut ptrs = Vec::new();
    for i in 0..16 {
        ptrs.push(allocator.try_alloc(i).unwrap().as_ptr());
    }
    assert_eq!(allocator.arenas_len(), 1);
    // heap_size counts only live arenas, so capture it while one is active.
    let heap_while_live = allocator.heap_size();
    assert_eq!(heap_while_live, 512);

    for ptr in ptrs {
        unsafe { allocator.mark_dropped(ptr.as_ptr() as *const u8) };
    }
    allocator.drop_dead_arenas();

    // After recycling, the arena is parked, no live arenas, so heap_size is 0.
    assert_eq!(allocator.arenas_len(), 0);
    assert_eq!(allocator.heap_size(), 0);
    // recycled_count == 1 proves the arena was parked in the recycle slot, not freed to the OS.
    assert_eq!(allocator.recycled_count, 1);

    // Allocate again, must reuse the recycled arena without growing OS footprint.
    // heap_size returns to the same value as when a live arena was present.
    for i in 16..32 {
        let _ = allocator.try_alloc(i).unwrap();
    }
    assert_eq!(allocator.arenas_len(), 1);
    assert_eq!(allocator.heap_size(), heap_while_live);
    // recycled_count == 0 proves the recycled slot was consumed rather than a new OS allocation.
    assert_eq!(allocator.recycled_count, 0);
}

#[test]
fn max_recycled_cap_respected() {
    let mut allocator = ArenaAllocator::default().with_arena_size(128);

    let mut ptrs_per_arena: Vec<Vec<NonNull<ArenaHeapItem<u64>>>> = Vec::new();

    for _ in 0..5 {
        let mut ptrs = Vec::new();
        let target_len = allocator.arenas_len() + 1;
        while allocator.arenas_len() < target_len {
            ptrs.push(allocator.try_alloc(0u64).unwrap().as_ptr());
        }
        ptrs_per_arena.push(ptrs);
    }
    assert_eq!(allocator.arenas_len(), 5);

    for ptrs in ptrs_per_arena {
        for ptr in ptrs {
            unsafe { allocator.mark_dropped(ptr.as_ptr() as *const u8) };
        }
    }

    allocator.drop_dead_arenas();

    assert_eq!(allocator.arenas_len(), 0);
    assert_eq!(allocator.heap_size(), 0);
    // The recycled list holds exactly max_recycled pages.
    assert_eq!(allocator.recycled_count, 4);
}

// === test for counter based drop tracking === //

// With counter based tracking instead of linkedlist, verify that
// alloc_count and drop_count are properly tracked.
#[test]
fn counter_based_drop_tracking() {
    let mut allocator = ArenaAllocator::default();

    let ptr_a = allocator.try_alloc(1u64).unwrap().as_ptr();
    let ptr_b = allocator.try_alloc(2u64).unwrap().as_ptr();
    let _ptr_c = allocator.try_alloc(3u64).unwrap().as_ptr();
    assert_eq!(allocator.arenas_len(), 1);

    // Mark A and B as dropped (don't mark C)
    unsafe {
        allocator.mark_dropped(ptr_a.as_ptr() as *const u8);
        allocator.mark_dropped(ptr_b.as_ptr() as *const u8);
    }

    // Arena should NOT be recyclable yet (C is still live)
    allocator.drop_dead_arenas();
    assert_eq!(allocator.arenas_len(), 1, "arena should still be live");
}

// === test for Dynamic Alignment === //

#[test]
fn test_over_aligned_type() {
    #[repr(C, align(512))]
    struct HighlyAligned {
        _data: [u8; 128],
    }

    let mut allocator = ArenaAllocator::default().with_arena_size(4096);
    let ptr = allocator
        .try_alloc(HighlyAligned { _data: [0; 128] })
        .unwrap();

    let addr = ptr.as_ptr().as_ptr() as usize;
    assert_eq!(addr % 512, 0);
    assert_eq!(allocator.arenas_len(), 1);
}

#[test]
fn test_alignment_upgrade_after_small_alloc() {
    #[repr(C, align(512))]
    struct BigAlign([u8; 16]);

    let mut allocator = ArenaAllocator::default().with_arena_size(4096);

    // force the first arena to use 8-byte alignment
    let _small = allocator.try_alloc(0u8).unwrap();
    assert_eq!(allocator.arenas_len(), 1);

    let ptr = allocator.try_alloc(BigAlign([0; 16])).unwrap();

    let addr = ptr.as_ptr().as_ptr() as usize;
    assert_eq!(addr % 512, 0);
    assert_eq!(allocator.arenas_len(), 2);
}

#[test]
fn test_alignment_upgrade_on_full_arena() {
    #[repr(C, align(512))]
    struct BigAlign([u8; 16]);

    let mut allocator = ArenaAllocator::default().with_arena_size(4096);

    // fill the first arena
    let mut count = 0usize;
    while allocator.arenas_len() < 2 {
        let _ = allocator.try_alloc(0u64).unwrap();
        count += 1;
        assert!(count < 1024);
    }

    let ptr = allocator.try_alloc(BigAlign([0; 16])).unwrap();

    let addr = ptr.as_ptr().as_ptr() as usize;
    assert_eq!(addr % 512, 0);
    assert_eq!(allocator.arenas_len(), 3);
}

// === test for transparent wrapper overhead === //

#[test]
fn arena_heap_item_is_transparent() {
    // Verify that ArenaHeapItem<T> has the same size as T
    // This proves we eliminated the 8 byte per allocation overhead
    assert_eq!(
        core::mem::size_of::<ArenaHeapItem<u64>>(),
        core::mem::size_of::<u64>(),
        "ArenaHeapItem should be transparent (same size as inner type)"
    );

    assert_eq!(
        core::mem::size_of::<ArenaHeapItem<[u8; 128]>>(),
        core::mem::size_of::<[u8; 128]>(),
        "ArenaHeapItem should be transparent for larger types too"
    );

    // Verify alignment is preserved
    assert_eq!(
        core::mem::align_of::<ArenaHeapItem<u64>>(),
        core::mem::align_of::<u64>(),
        "ArenaHeapItem should preserve alignment"
    );
}
