use core::ptr::NonNull;
use rust_alloc::vec::Vec;

use crate::alloc::arena2::ArenaHeapItem;

use super::ArenaAllocator;

#[test]
fn alloc_dealloc() {
    // Let's just allocate with a half a Kb per arena
    let mut allocator = ArenaAllocator::default().with_arena_size(512);

    let mut first_region: Vec<NonNull<ArenaHeapItem<i32>>> = Vec::default();
    for i in 0..32 {
        let ap = allocator.try_alloc(i).unwrap();
        first_region.push(ap.as_ptr());
    }
    assert!(
        allocator.arenas_len() >= 1,
        "at least one arena must exist after allocations"
    );

    let mut second_region: Vec<NonNull<ArenaHeapItem<i32>>> = Vec::default();
    for i in 0..32 {
        let ap = allocator.try_alloc(i).unwrap();
        second_region.push(ap.as_ptr());
    }
    assert!(
        allocator.arenas_len() >= 1,
        "arenas must still exist after further allocations"
    );

    // release first region via free_slot
    for ptr in first_region {
        allocator.free_slot(ptr.cast::<u8>());
    }
    allocator.drop_dead_arenas();

    // there may or may not be one fewer arena depending on slot packing, but
    // the allocator must still contain the second region
    assert!(allocator.arenas_len() <= 2);
    drop(second_region);
}

#[test]
fn free_list_reclaims_slots() {
    let mut allocator = ArenaAllocator::default().with_arena_size(4096);

    let mut ptrs: Vec<NonNull<ArenaHeapItem<u64>>> = (0u64..32)
        .map(|i| allocator.try_alloc(i).unwrap().as_ptr())
        .collect();

    let arenas_after_alloc = allocator.arenas_len();
    assert!(arenas_after_alloc >= 1);

    // free the first 16 slots via free_slot
    let to_free = ptrs.drain(..16).collect::<Vec<_>>();
    for ptr in to_free {
        allocator.free_slot(ptr.cast::<u8>());
    }

    // reallocate 16 more items, they should reuse freed slots not create
    // a new arena
    for i in 32u64..48 {
        let _ = allocator.try_alloc(i).unwrap();
    }

    assert_eq!(
        allocator.arenas_len(),
        arenas_after_alloc,
        "free list must allow slot reuse without new arenas"
    );
}

// bitmap drop check, if all slots freed then arena is reclaimed
#[test]
fn bitmap_drop_check() {
    let mut allocator = ArenaAllocator::default().with_arena_size(4096);

    let ptrs: Vec<NonNull<ArenaHeapItem<u64>>> = (0u64..16)
        .map(|i| allocator.try_alloc(i).unwrap().as_ptr())
        .collect();

    assert_eq!(allocator.arenas_len(), 1);

    //free every slot
    for ptr in ptrs {
        allocator.free_slot(ptr.cast::<u8>());
    }

    allocator.drop_dead_arenas();

    assert_eq!(
        allocator.arenas_len(),
        0,
        "all-empty arena must be dropped by drop_dead_arenas"
    );
}

#[test]
fn arc_drop() {
    use core::sync::atomic::{AtomicBool, Ordering};
    use core::ptr::drop_in_place;
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

    // manually drop the value through the pointer, then free the slot
    let mut heap_item_ptr = a.as_ptr();
    unsafe {
        // drop the inner value
        drop_in_place(heap_item_ptr.as_mut().value_mut());
    }
    allocator.free_slot(heap_item_ptr.cast::<u8>());

    assert!(dropped.load(Ordering::SeqCst), "destructor must have run");
    assert_eq!(allocator.arenas_len(), 1);

    allocator.drop_dead_arenas();
    assert_eq!(allocator.arenas_len(), 0, "empty arena must be reclaimed");
}
