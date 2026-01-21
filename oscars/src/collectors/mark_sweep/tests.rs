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
