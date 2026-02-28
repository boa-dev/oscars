use crate::collectors::mark_sweep::MarkSweepGarbageCollector;
use crate::{Finalize, Trace};

use super::Gc;
use super::WeakMap;
use super::cell::GcRefCell;
use crate::Root;

#[test]
fn basic_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(64)
        .with_heap_threshold(128);

    let gc = Root::new_in(GcRefCell::new(10), collector);

    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    collector.collect();

    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    assert_eq!(*gc.borrow(), 10);
}

#[test]
fn nested_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(128)
        .with_heap_threshold(512);

    // With size-class routing, objects of different types may land in different
    // arenas, we assert at least one arena not an exact count

    let gc_root = Root::new_in(GcRefCell::new(10), collector);
    let gc = gc_root.into_gc();

    let nested_gc = Root::new_in(gc.clone(), collector);
    let initial_arenas = collector.allocator.borrow().arenas_len();
    assert!(initial_arenas >= 1);

    drop(gc_root);
    collector.collect();

    // after collecting dead gc_root, arena count must not grow
    // freed slots are returned to the free list, not removed as separate arenas
    let after_collect_arenas = collector.allocator.borrow().arenas_len();
    assert!(
        after_collect_arenas <= initial_arenas,
        "arenas must not grow after collecting dead objects"
    );
    assert_eq!(*nested_gc.borrow(), 10);

    let new_gc = Root::new_in(GcRefCell::new(8), collector);

    // one more live object may reuse a free list slot or add to an existing arena.
    let after_second_alloc = collector.allocator.borrow().arenas_len();
    assert!(
        after_second_alloc >= after_collect_arenas,
        "arena count must not decrease on allocation"
    );

    drop(new_gc);
    collector.collect();

    // after collecting new_gc, arena count must not exceed
    let final_arenas = collector.allocator.borrow().arenas_len();
    assert!(
        final_arenas <= after_second_alloc,
        "dead objects must not keep arenas alive"
    );
    assert_eq!(*nested_gc.borrow(), 10);
}

#[test]
fn gc_recursion() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(4096)
        .with_heap_threshold(8_192);

    #[derive(Debug, Finalize, Trace)]
    struct S {
        i: usize,
        next: Option<Gc<S>>,
    }

    const COUNT: usize = 2_000;

    let mut root_handle = Root::new_in(S { i: 0, next: None }, collector);
    for i in 1..COUNT {
        root_handle = Root::new_in(
            S {
                i,
                next: Some(root_handle.into_gc()),
            },
            collector,
        );
    }

    drop(root_handle);
    collector.collect();
}

#[test]
fn drop_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Root::new_in(GcRefCell::new(7u64), collector);
    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    collector.collect();
    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    drop(gc);
    collector.collect();

    // TODO: don't drop an active arena
    assert_eq!(collector.allocator.borrow().arenas_len(), 0, "arena not freed");
}

#[test]
fn clone_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Root::new_in(GcRefCell::new(42u32), collector);
    let gc_clone = gc.clone();

    drop(gc);
    collector.collect();

    assert_eq!(*gc_clone.borrow(), 42u32, "collected despite live clone");
}

#[test]
fn multi_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(128)
        .with_heap_threshold(512);

    for _ in 0..3 {
        let objects: rust_alloc::vec::Vec<_> = (0..4)
            .map(|i| Root::new_in(GcRefCell::new(i as u64), collector))
            .collect();

        assert!(collector.allocator.borrow().arenas_len() >= 1);

        drop(objects);
        collector.collect();

        assert_eq!(collector.allocator.borrow().arenas_len(), 0, "arenas not reclaimed");
    }
}

#[test]
fn pressure_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(128)
        .with_heap_threshold(256);

    let root = Root::new_in(GcRefCell::new(99u64), collector);

    // Keeping all temporaries alive at once so the allocator hits the threshold
    // and fires a collection while root is still live
    let _temporaries: rust_alloc::vec::Vec<_> = (0..20u64)
        .map(|i| Root::new_in(GcRefCell::new(i), collector))
        .collect();

    assert_eq!(*root.borrow(), 99u64, "root collected under pressure");
}

#[test]
fn borrow_mut_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Root::new_in(GcRefCell::new(0u64), collector);
    *gc.borrow_mut() = 42;

    collector.collect();

    assert_eq!(*gc.borrow(), 42u64, "mutation lost after collect");
}

#[test]
fn long_lived_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Root::new_in(GcRefCell::new(77u64), collector);

    for _ in 0..10 {
        collector.collect();
    }

    assert_eq!(*gc.borrow(), 77u64, "swept during color-flip");
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        1,
        "arena freed while live"
    );
}

#[test]
fn basic_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(42u64, collector);

    map.insert(&key, 100u64, collector);

    assert_eq!(map.get(&key), Some(&100u64));
    assert!(map.is_key_alive(&key));
}

    #[test]
    fn basic_alloc() {
        let gc = MarkSweepGarbageCollector::default();
        let layout = Layout::from_size_align(32, 8).unwrap();
        let block = gc.allocate(layout).expect("allocation should succeed");
        assert_eq!(block.len(), 32);
        // deallocate does nothing but should not crash
        unsafe { gc.deallocate(block.cast(), layout) };
    }

    // vec can use the gc reference as its allocator
    #[test]
    fn vec_alloc() {
        let gc = MarkSweepGarbageCollector::default();
        let mut v: GcVec<u64, &MarkSweepGarbageCollector> = GcVec::new_in(&gc);
        for i in 0..8u64 {
            v.push(i);
        }
        assert_eq!(v.len(), 8);
        assert_eq!(v[7], 7);
        // v and gc use the same arena
        assert!(gc.allocator.borrow().arenas_len() > 0);
    }

    // skipping the borrow checker with unsafe
    //
    // we use a raw pointer to bypass the compiler rules
    // Warning: if collect() frees memory you are still using, it will crash.
    #[test]
    fn unsafe_collect() {
        let gc = MarkSweepGarbageCollector::default();
        let mut v: GcVec<u64, &MarkSweepGarbageCollector> = GcVec::new_in(&gc);
        v.push(1u64);

        // bypass borrow checker with a raw pointer
        unsafe {
            // we use a raw pointer to satisfy the mutable call
            let ptr = &gc as *const _ as *mut MarkSweepGarbageCollector;
            (*ptr).collect();
        }

        assert_eq!(v.len(), 1);
        assert_eq!(v[0], 1);
        drop(v);
    }

    // check alignment limits
    #[test]
    fn over_aligned_succeeds() {
        let gc = MarkSweepGarbageCollector::default();
        let layout = Layout::from_size_align(64, 32).unwrap();
        assert!(gc.allocate(layout).is_ok(), "alignment > 16 should succeed now");
    }

    #[test]
    fn zst_alloc() {
        let gc = MarkSweepGarbageCollector::default();
        let layout = Layout::new::<()>();
        let block = gc.allocate(layout).expect("zst alloc should work");
        assert_eq!(block.len(), 0);
        unsafe { gc.deallocate(block.cast(), layout) };
    }

    #[test]
    fn vec_grow_reclaims_memory() {
        let gc = MarkSweepGarbageCollector::default().with_arena_size(1024).with_heap_threshold(10240);
        
        let mut v: GcVec<u64, &MarkSweepGarbageCollector> = GcVec::new_in(&gc);
        
        // push items to force vector reallocations
        // old buffers should be deallocated as it grows
        for i in 0..100u64 {
            v.push(i);
        }
        
        drop(v);

        gc.collect();
        
        // typed arenas may persist for the size-class pool, but raw arenas must be 0
        let raw_arena_count = gc.allocator.borrow().raw_arenas.len();
        assert_eq!(
            raw_arena_count, 0,
            "Raw arenas were not reclaimed after Vector was dropped. Memory leak!"
        );
    }

    #[test]
    fn vec_triggers_gc_threshold() {
        let gc = MarkSweepGarbageCollector::default().with_arena_size(256).with_heap_threshold(1024);
        
        let mut v: GcVec<u64, &MarkSweepGarbageCollector> = GcVec::new_in(&gc);
        
        // force allocations exceeding the threshold to trigger collect_needed
        for i in 0..500u64 {
            v.push(i);
        }
        
        assert!(v.len() > 0);
    }
}
