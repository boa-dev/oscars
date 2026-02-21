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
fn basic_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new();
    collector.register_weak_map(core::ptr::addr_of_mut!(map));
    let key = Gc::new_in(42u64, collector);

    map.insert(&key, 100u64, collector);

    assert_eq!(map.get(&key), Some(&100u64));
    assert!(map.is_key_alive(&key));
    collector.unregister_weak_map(core::ptr::addr_of_mut!(map));
}

#[test]
fn dead_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new();
    collector.register_weak_map(core::ptr::addr_of_mut!(map));
    let key = Gc::new_in(42u64, collector);

    map.insert(&key, 100u64, collector);
    assert_eq!(map.get(&key), Some(&100u64));

    drop(key);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 0, "ephemeron not swept");
    collector.unregister_weak_map(core::ptr::addr_of_mut!(map));
}

#[test]
fn update_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new();
    collector.register_weak_map(core::ptr::addr_of_mut!(map));
    let key = Gc::new_in(1u64, collector);

    // insert then update so that old value doesn't leak
    map.insert(&key, 10u64, collector);
    map.insert(&key, 20u64, collector);

    assert_eq!(map.get(&key), Some(&20u64), "value not updated");

    drop(key);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 0, "arena leaked after update");
    collector.unregister_weak_map(core::ptr::addr_of_mut!(map));
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
            _map: WeakMap::new(),
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

    let mut map = WeakMap::new();
    collector.register_weak_map(core::ptr::addr_of_mut!(map));
    let key = Gc::new_in(1u64, collector);

    map.insert(&key, 99u64, collector);
    assert_eq!(map.get(&key), Some(&99u64));

    // remove should return the value and leave map empty
    let removed = map.remove(&key);
    assert_eq!(removed, Some(99u64), "remove returned wrong value");
    assert_eq!(map.get(&key), None, "entry still present after remove");
    collector.unregister_weak_map(core::ptr::addr_of_mut!(map));
}

#[test]
fn prune_wm() {
    //  dangling pointer fix
    // ensure insert doesn't read freed memory on dead entries
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new();
    // register so the collector prunes dead entries before freeing arenas
    collector.register_weak_map(core::ptr::addr_of_mut!(map));

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
    collector.unregister_weak_map(core::ptr::addr_of_mut!(map));
}

#[test]
fn remove_then_collect() {
    // ensure remove() doesn't leak the backing ephemeron after key is gone
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new();
    collector.register_weak_map(core::ptr::addr_of_mut!(map));
    let key = Gc::new_in(1u64, collector);

    map.insert(&key, 99u64, collector);
    let removed = map.remove(&key);
    assert_eq!(removed, Some(99u64));

    // the ephemeron stays in the queue until the key is collected
    drop(key);
    collector.collect();

    assert_eq!(collector.allocator.arenas_len(), 0, "ephemeron leaked after remove");
    collector.unregister_weak_map(core::ptr::addr_of_mut!(map));
}
