use oscars::{Gc, WeakGc, WeakMap};

// Compile-time regression guards: if any GC handle type ever becomes
// Send or Sync again, these lines will fail to compile.
static_assertions::assert_not_impl_any!(Gc<String>: Send, Sync);
static_assertions::assert_not_impl_any!(WeakGc<String>: Send, Sync);
static_assertions::assert_not_impl_any!(WeakMap<String, String>: Send, Sync);

#[test]
fn gc_works_on_single_thread() {
    let collector = &mut oscars::MarkSweepGarbageCollector::default()
        .with_arena_size(64)
        .with_heap_threshold(128);
    let gc = Gc::new_in(7u64, collector);
    collector.collect();
    assert_eq!(*gc, 7u64);
}

