#[allow(dead_code)]
fn assert_not_send<T: Send>() {}

#[test]
fn gc_must_not_be_send() {
    // assert_not_send::<oscars::Gc<String>>();
}

#[test]
fn gc_works_on_single_thread() {
    let collector = &mut oscars::MarkSweepGarbageCollector::default()
        .with_arena_size(64)
        .with_heap_threshold(128);
    let gc = oscars::Gc::new_in(7u64, collector);
    collector.collect();
    assert_eq!(*gc, 7u64);
}
