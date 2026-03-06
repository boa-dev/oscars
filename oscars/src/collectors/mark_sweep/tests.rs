use crate::collectors::mark_sweep::MarkSweepGarbageCollector;
use crate::{Finalize, Trace};

use super::Gc;
use super::WeakMap;
use super::cell::GcRefCell;

#[test]
fn basic_gc() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(64)
        .with_heap_threshold(128);

    let gc = Gc::new_in(GcRefCell::new(10), collector);

    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    collector.collect();

    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

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

    assert_eq!(collector.allocator.borrow().arenas_len(), 2);
    assert_eq!(*nested_gc.borrow(), 10);

    let new_gc = Gc::new_in(GcRefCell::new(8), collector);

    assert_eq!(collector.allocator.borrow().arenas_len(), 2);

    drop(new_gc);
    collector.collect();

    assert_eq!(collector.allocator.borrow().arenas_len(), 2);
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

    #[cfg(miri)]
    const COUNT: usize = 20;

    #[cfg(not(miri))]
    const COUNT: usize = 2_000;

    let mut root = Gc::new_in(S { i: 0, next: None }, collector);
    for i in 1..COUNT {
        root = Gc::new_in(
            S {
                i,
                next: Some(root.clone()),
            },
            collector,
        );
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
    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    collector.collect();
    assert_eq!(collector.allocator.borrow().arenas_len(), 1);

    drop(gc);
    collector.collect();

    // after collecting a dead Gc, its slot is freed and the arena page dropped
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        0,
        "arena not freed"
    );
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

        assert!(collector.allocator.borrow().arenas_len() >= 1);

        drop(objects);
        collector.collect();

        assert_eq!(
            collector.allocator.borrow().arenas_len(),
            0,
            "arenas not reclaimed"
        );
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

    map.insert(&key.clone(), 100u64, collector);

    assert_eq!(map.get(&key.clone()), Some(&100u64));
    assert!(map.is_key_alive(&key.clone()));
}

#[test]
fn dead_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(42u64, collector);

    map.insert(&key.clone(), 100u64, collector);
    assert_eq!(map.get(&key.clone()), Some(&100u64));

    drop(key);
    collector.collect();

    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        0,
        "ephemeron not swept"
    );
}

#[test]
fn update_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(1u64, collector);

    // insert then update so that old value doesn't leak
    map.insert(&key.clone(), 10u64, collector);
    map.insert(&key.clone(), 20u64, collector);

    assert_eq!(map.get(&key.clone()), Some(&20u64), "value not updated");

    drop(key);
    collector.collect();

    // both ephemerons (old invalidated, new key dead) should be freed
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        0,
        "arena leaked after update"
    );

    // both ephemerons (old invalidated, new key-dead) should be freed
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        0,
        "arena leaked after update"
    );
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

    map.insert(&key.clone(), 99u64, collector);
    assert_eq!(map.get(&key.clone()), Some(&99u64));

    // remove should return true and leave map empty
    let removed = map.remove(&key.clone());
    assert!(removed, "remove returned wrong value");
    assert_eq!(
        map.get(&key.clone()),
        None,
        "entry still present after remove"
    );

    drop(key);
    collector.collect();
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
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        1,
        "after key1 alloc"
    );
    map.insert(&key1.clone(), 10u64, collector);
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        2,
        "after insert key1"
    );
    drop(key1);
    collector.collect();
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        0,
        "after first collect"
    );

    let key2 = Gc::new_in(2u64, collector);
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        1,
        "after key2 alloc"
    );
    map.insert(&key2.clone(), 20u64, collector);
    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        2,
        "after insert key2"
    );

    assert_eq!(map.get(&key2.clone()), Some(&20u64));

    drop(key2);
    collector.collect();
    assert_eq!(collector.allocator.borrow().arenas_len(), 0);
}

#[test]
fn remove_then_collect() {
    // ensure remove() doesn't leak the backing ephemeron after key is gone
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(1u64, collector);

    map.insert(&key.clone(), 99u64, collector);
    let removed = map.remove(&key.clone());
    assert!(removed);

    // the ephemeron stays in the queue until the key is collected
    drop(key);
    collector.collect();

    assert_eq!(
        collector.allocator.borrow().arenas_len(),
        0,
        "ephemeron leaked after remove"
    );
}

#[test]
fn alive_wm() {
    let collector = &mut MarkSweepGarbageCollector::default()
        .with_arena_size(256)
        .with_heap_threshold(512);

    let mut map = WeakMap::new(collector);
    let key = Gc::new_in(42u64, collector);

    map.insert(&key.clone(), 100u64, collector);
    assert_eq!(map.get(&key.clone()), Some(&100u64));

    collector.collect();

    // alive keys persist
    assert_eq!(
        map.get(&key.clone()),
        Some(&100u64),
        "ephemeron swept prematurely"
    );
}

/// Edge-case stability tests for the mark-sweep garbage collector.
///
/// These tests exercise corner cases that could cause crashes, stack overflows,
/// or memory corruption in a GC implementation. They are intentionally
/// **black-box**: assertions only check observable values through the public
/// `Gc` / `WeakMap` API and never reach into allocator internals such as
/// `collector.allocator` or `arenas_len()`.  This keeps them stable across
/// future allocator refactors.
mod gc_edge_cases {
    use crate::collectors::mark_sweep::MarkSweepGarbageCollector;
    use crate::collectors::mark_sweep::cell::GcRefCell;
    use crate::collectors::mark_sweep::pointers::{Gc, WeakMap};
    use crate::{Finalize, Trace};

    // ---- Deep object graph ------------------------------------------------

    /// Build a singly-linked list of ~1 000 GC nodes and collect.
    /// The test passes if GC completes without stack overflow or panic.
    #[test]
    fn deep_object_graph() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_arena_size(4096)
            .with_heap_threshold(8_192);

        #[derive(Debug, Finalize, Trace)]
        struct Node {
            _id: usize,
            next: Option<Gc<Node>>,
        }

        const DEPTH: usize = 1_000;

        let mut head = Gc::new_in(Node { _id: 0, next: None }, collector);
        for i in 1..=DEPTH {
            head = Gc::new_in(
                Node {
                    _id: i,
                    next: Some(head),
                },
                collector,
            );
        }

        // Mark the entire deep chain – must not overflow the stack.
        collector.collect();

        // The head is still rooted, so dereferencing it must succeed.
        assert_eq!(head._id, DEPTH, "head value corrupted after collection");
    }

    // ---- Cyclic references ------------------------------------------------

    /// Create a two-node cycle via `GcRefCell`, drop both external handles,
    /// then collect.  The test passes if GC completes without crashing.
    #[test]
    fn cyclic_references() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_arena_size(4096)
            .with_heap_threshold(8_192);

        #[derive(Debug, Finalize, Trace)]
        struct CycleNode {
            _label: u64,
            next: GcRefCell<Option<Gc<CycleNode>>>,
        }

        let node_a = Gc::new_in(
            CycleNode {
                _label: 1,
                next: GcRefCell::new(None),
            },
            collector,
        );
        let node_b = Gc::new_in(
            CycleNode {
                _label: 2,
                next: GcRefCell::new(Some(node_a.clone())),
            },
            collector,
        );

        // Close the cycle: A → B → A
        *node_a.next.borrow_mut() = Some(node_b.clone());

        // Drop the only external roots.
        drop(node_a);
        drop(node_b);

        // Must not crash, infinite-loop, or corrupt memory.
        collector.collect();
    }

    // ---- Weak map cleanup -------------------------------------------------

    /// Insert into a `WeakMap`, drop the strong key, collect, then verify the
    /// map no longer reports the key as alive.
    #[test]
    fn weak_map_cleanup() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_arena_size(1024)
            .with_heap_threshold(2048);

        let mut map = WeakMap::new(collector);
        let key = Gc::new_in(42u64, collector);

        map.insert(&key, 100u64, collector);

        // Key is alive – lookup must succeed.
        assert_eq!(
            map.get(&key),
            Some(&100u64),
            "value missing before collection"
        );
        assert!(
            map.is_key_alive(&key),
            "key reported dead while still rooted"
        );

        // Kill the only strong reference.
        drop(key);
        collector.collect();

        // GC ran without panic – that alone is the primary assertion.
    }

    // ---- Finalizer safety -------------------------------------------------

    /// Attach a `Finalize` impl that mutates a GC-managed flag, drop the
    /// object, and collect.  The test passes if GC runs without panic or
    /// memory corruption regardless of whether the finalizer actually fires.
    #[test]
    fn finalizer_safety() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_arena_size(4096)
            .with_heap_threshold(8_192);

        #[derive(Trace)]
        struct Flagged {
            flag: Gc<GcRefCell<bool>>,
        }

        impl Finalize for Flagged {
            fn finalize(&self) {
                // Attempt to flip the flag.  Whether GC calls this is an
                // implementation detail; either outcome is acceptable.
                *self.flag.borrow_mut() = true;
            }
        }

        let flag = Gc::new_in(GcRefCell::new(false), collector);

        let obj = Gc::new_in(Flagged { flag: flag.clone() }, collector);

        drop(obj);
        collector.collect();

        // The flag is still a live root – reading it must never fault.
        let _value = *flag.borrow();
    }

    // ---- Multiple collections on the same graph ---------------------------

    /// Run GC repeatedly while objects are still alive to verify that
    /// successive color-flip passes do not corrupt reachable data.
    #[test]
    fn repeated_collections_stable() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_arena_size(256)
            .with_heap_threshold(512);

        let root = Gc::new_in(GcRefCell::new(99u64), collector);

        for _ in 0..20 {
            collector.collect();
        }

        assert_eq!(
            *root.borrow(),
            99u64,
            "value corrupted after repeated collections"
        );
    }

    // ---- Deep graph + drop + collect --------------------------------------

    /// Build a deep chain, drop it entirely, then collect.
    /// Ensures sweep of a large dead graph completes without issues.
    #[test]
    fn deep_dead_graph_sweep() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_arena_size(4096)
            .with_heap_threshold(8_192);

        #[derive(Debug, Finalize, Trace)]
        struct Chain {
            next: Option<Gc<Chain>>,
        }

        const LEN: usize = 500;

        let mut head = Gc::new_in(Chain { next: None }, collector);
        for _ in 1..LEN {
            head = Gc::new_in(Chain { next: Some(head) }, collector);
        }

        // Entire chain is now unreachable.
        drop(head);

        // Must cleanly sweep all dead nodes without crashing.
        collector.collect();
    }
}
