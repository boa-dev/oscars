use crate::{Finalize, Trace};

use crate::collectors::mark_sweep::MarkSweepGarbageCollector;

use super::Gc;
use super::cell::GcRefCell;

#[test]
fn basic_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(64)
        .with_heap_threshold(128);

    let gc = Gc::new_in(GcRefCell::new(10), collector);

    assert_eq!(collector.allocator.arenas_len(), 1);

    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 1);

    assert_eq!(*gc.borrow(), 10);
}

#[test]
fn nested_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(80)
        .with_heap_threshold(128);

    // We are allocating 32 bytes, per GC, which with the linked list pointer should be
    // 36 or 40 bytes depending on the system.

    let gc = Gc::new_in(GcRefCell::new(10), collector);

    let nested_gc = Gc::new_in(gc.clone(), collector);

    drop(gc);

    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 1);
    assert_eq!(*nested_gc.borrow(), 10);

    let new_gc = Gc::new_in(GcRefCell::new(8), collector);

    assert_eq!(collector.allocator.arenas_len(), 2);

    drop(new_gc);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 1);
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

    let mut root = Gc::new_in(S { i: 0, next: None }, collector);
    for i in 1..COUNT {
        root = Gc::new_in(S {
                i,
                next: Some(root),
        }, collector);
    }

    drop(root);
    collector.collect();
}

#[test]
fn drop_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Gc::new_in(GcRefCell::new(7u64), collector);
    assert_eq!(collector.allocator.arenas_len(), 1);

    collector.collect();
    assert_eq!(collector.allocator.arenas_len(), 1);

    drop(gc);
    collector.collect();

	// TODO: don't drop an active arena
    assert_eq!(collector.allocator.arenas_len(), 0, "arena not freed");
}

#[test]
fn clone_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Gc::new_in(GcRefCell::new(42u32), collector);
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
            .map(|i| Gc::new_in(GcRefCell::new(i as u64), collector))
            .collect();

        assert!(collector.allocator.arenas_len() >= 1);

        drop(objects);
        collector.collect();

        assert_eq!(collector.allocator.arenas_len(), 0, "arenas not reclaimed");
    }
}

#[test]
fn pressure_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(128)
        .with_heap_threshold(256);

    let root = Gc::new_in(GcRefCell::new(99u64), collector);

    // Keeping all temporaries alive at once so the allocator hits the threshold
    // and fires a collection while root is still live
    let _temporaries: rust_alloc::vec::Vec<_> = (0..20u64)
        .map(|i| Gc::new_in(GcRefCell::new(i), collector))
        .collect();

    assert_eq!(*root.borrow(), 99u64, "root collected under pressure");
}

#[test]
fn borrow_mut_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Gc::new_in(GcRefCell::new(0u64), collector);
    *gc.borrow_mut() = 42;

    collector.collect();

    assert_eq!(*gc.borrow(), 42u64, "mutation lost after collect");
}

#[test]
fn long_lived_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let gc = Gc::new_in(GcRefCell::new(77u64), collector);

    for _ in 0..10 {
        collector.collect();
    }

    assert_eq!(*gc.borrow(), 77u64, "swept during color-flip");
    assert_eq!(
        collector.allocator.arenas_len(),
        1,
        "arena freed while live"
    );
}
