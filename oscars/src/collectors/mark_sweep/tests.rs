use crate::{Finalize, Trace};

use crate::collectors::mark_sweep::MarkSweepGarbageCollector;

use super::Gc;
use super::WeakMap;
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
fn dead_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(42u64, collector);

    map.insert(&key, 100u64, collector);
    assert_eq!(map.get(&key), Some(&100u64));

    drop(key);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 0, "ephemeron not swept");
}

#[test]
fn update_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(1u64, collector);

    // insert then update so that old value doesn't leak
    map.insert(&key, 10u64, collector);
    map.insert(&key, 20u64, collector);

    assert_eq!(map.get(&key), Some(&20u64), "value not updated");

    drop(key);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 0, "arena leaked after update");
}

#[test]
fn trace_wm() {
    // weak_map must implement Trace to be embeddable in traced structs
    #[derive(Finalize, Trace)]
    struct Container {
        _map: WeakMap<u64, u64>,
    }

    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let container = Gc::new_in(
        Container {
            _map: WeakMap::new(collector),
        },
        collector,
    );

    collector.collect();

    drop(container);
}

#[test]
fn remove_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(1u64, collector);

    map.insert(&key, 99u64, collector);
    assert_eq!(map.get(&key), Some(&99u64));

    // remove should return the value and leave map empty
    let removed = map.remove(&key);
    assert_eq!(removed, Some(99u64), "remove returned wrong value");
    assert_eq!(map.get(&key), None, "entry still present after remove");
}

#[test]
fn prune_wm() {
    //  dangling pointer fix
    // ensure insert doesn't read freed memory on dead entries
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);

    let key1 = Gc::new_in(1u64, collector);
    assert_eq!(collector.allocator.arenas_len(), 1, "after key1 alloc");
    map.insert(&key1, 10u64, collector);
    assert_eq!(collector.allocator.arenas_len(), 1, "after insert key1");
    drop(key1);
    collector.collect();
    assert_eq!(collector.allocator.arenas_len(), 0, "after first collect");

    let key2 = Gc::new_in(2u64, collector);
    assert_eq!(collector.allocator.arenas_len(), 1, "after key2 alloc");
    map.insert(&key2, 20u64, collector);
    assert_eq!(collector.allocator.arenas_len(), 1, "after insert key2");

    assert_eq!(map.get(&key2), Some(&20u64));

    drop(key2);
    collector.collect();
    assert_eq!(collector.allocator.arenas_len(), 0);
}

#[test]
fn remove_then_collect() {
    // ensure remove() doesn't leak the backing ephemeron after key is gone
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(1u64, collector);

    map.insert(&key, 99u64, collector);
    let removed = map.remove(&key);
    assert_eq!(removed, Some(99u64));

    // the ephemeron stays in the queue until the key is collected
    drop(key);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 0, "ephemeron leaked after remove");
}

#[test]
fn alive_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(42u64, collector);

    map.insert(&key, 100u64, collector);
    assert_eq!(map.get(&key), Some(&100u64));

    collector.collect();

    // alive keys persist
    assert_eq!(map.get(&key), Some(&100u64), "ephemeron swept prematurely");
}
