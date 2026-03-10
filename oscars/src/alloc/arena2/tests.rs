use core::ptr::{NonNull, drop_in_place};

use rust_alloc::vec::Vec;

use crate::alloc::arena2::ArenaHeapItem;

use super::ArenaAllocator;

// TODO: Needs testing on a 32bit system
#[test]
fn alloc_dealloc() {
    // Let's just allocate with a half a Kb per arena
    let mut allocator = ArenaAllocator::default().with_arena_size(512);

    // An Arena heap object has an overhead of 4-8 bytes, depending on the platform

    let mut first_region = Vec::default();
    for i in 0..32 {
        let value = allocator.try_alloc(i).unwrap();
        first_region.push(value.as_ptr());
    }
    assert_eq!(allocator.arenas_len(), 1);

    let mut second_region = Vec::default();
    for i in 0..32 {
        let value = allocator.try_alloc(i).unwrap();
        second_region.push(value.as_ptr());
    }
    assert_eq!(allocator.arenas_len(), 2);

    // Drop all the items in the first region
    manual_drop(first_region);

    // Drop dead pages
    allocator.drop_dead_arenas();

    assert_eq!(allocator.arenas_len(), 1);
}

fn manual_drop(region: Vec<NonNull<ArenaHeapItem<i32>>>) {
    for mut item in region {
        unsafe {
            item.as_mut().mark_dropped();
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
    let mut heap_item = a.as_ptr();
    unsafe {
        let heap_item_mut = heap_item.as_mut();
        // Manually drop the heap item
        heap_item_mut.mark_dropped();
        drop_in_place(ArenaHeapItem::as_value_ptr(heap_item));
    };

    assert!(dropped.load(Ordering::SeqCst));

    assert_eq!(allocator.arenas_len(), 1);

    allocator.drop_dead_arenas();

    assert_eq!(allocator.arenas_len(), 0);
}

// === test for TaggedPtr::as_ptr === //

// `TaggedPtr::as_ptr` must use `addr & !MASK` to unconditionally clear the high
// bit rather than XORing it out. The XOR approach worked for tagged items
// but incorrectly flipped the bit on untagged items, corrupting the pointer.
#[test]
fn as_ptr_clears_not_flips_tag_bit() {
    let mut allocator = ArenaAllocator::default();

    let mut ptr_a = allocator.try_alloc(1u64).unwrap().as_ptr();
    let mut ptr_b = allocator.try_alloc(2u64).unwrap().as_ptr();
    let _ptr_c = allocator.try_alloc(3u64).unwrap().as_ptr();
    assert_eq!(allocator.arenas_len(), 1);

    // Mark B and C as dropped, leave A live.
    unsafe {
        ptr_b.as_mut().mark_dropped();
    }

    let states = allocator.arena_drop_states();
    assert_eq!(
        states[0].as_slice(),
        &[false, true, false],
        "item_drop_states must correctly report live/dropped status for all nodes"
    );

    unsafe {
        ptr_a.as_mut().mark_dropped();
    }
}
