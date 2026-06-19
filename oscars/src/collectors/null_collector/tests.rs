use super::NullCollector;
use crate::collectors::mark_sweep::{
    Collector, Finalize, Gc, Trace, TraceColor, WeakGc, WeakMap, cell::GcRefCell,
};

#[test]
fn basic_alloc_and_read() {
    let nc = NullCollector::default();
    let gc = Gc::new_in(42u32, &nc);
    assert_eq!(
        *gc, 42u32,
        "value should be readable immediately after alloc"
    );
}

#[test]
fn collect_is_noop() {
    let nc = NullCollector::default();
    let gc = Gc::new_in(GcRefCell::new(7u64), &nc);

    nc.collect();
    nc.collect();
    nc.collect();

    assert_eq!(*gc.borrow(), 7u64, "value must survive collect() calls");
}

#[test]
fn gc_color_is_stable() {
    let nc = NullCollector::default();
    let color1 = nc.gc_color();
    nc.collect();
    let color2 = nc.gc_color();
    assert!(
        matches!((color1, color2), (TraceColor::White, TraceColor::White)),
        "gc_color must never flip on a NullCollector"
    );
}

#[test]
fn multiple_allocs() {
    let nc = NullCollector::default()
        .with_page_size(64)
        .with_heap_threshold(512);

    let gcs: rust_alloc::vec::Vec<_> = (0u64..8)
        .map(|i| Gc::new_in(GcRefCell::new(i), &nc))
        .collect();

    for (i, gc) in gcs.iter().enumerate() {
        assert_eq!(*gc.borrow(), i as u64, "value {i} changed unexpectedly");
    }
}

#[test]
fn drop_signals_finalizer() {
    use core::sync::atomic::{AtomicBool, Ordering};
    use rust_alloc::sync::Arc;

    let dropped = Arc::new(AtomicBool::new(false));

    struct Spy(Arc<AtomicBool>);
    impl Drop for Spy {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl Finalize for Spy {}
    // SAFETY: `Spy` has no GC children.
    unsafe impl Trace for Spy {
        crate::empty_trace!();
    }

    {
        let nc = NullCollector::default();
        let _gc = Gc::new_in(Spy(Arc::clone(&dropped)), &nc);
        assert!(!dropped.load(Ordering::SeqCst), "dropped too early");
    }

    assert!(
        dropped.load(Ordering::SeqCst),
        "Spy::drop must run when the NullCollector is dropped"
    );
}

#[test]
fn mutation_persists() {
    let nc = NullCollector::default();
    let gc = Gc::new_in(GcRefCell::new(0u64), &nc);

    *gc.borrow_mut() = 99;
    nc.collect();
    assert_eq!(*gc.borrow(), 99u64, "mutation lost after collect()");
}

#[test]
fn clone_shares_allocation() {
    let nc = NullCollector::default();
    let gc = Gc::new_in(GcRefCell::new(1u32), &nc);
    let gc2 = gc.clone();

    assert!(
        Gc::ptr_eq(&gc, &gc2),
        "clone must alias the same allocation"
    );
    *gc.borrow_mut() = 2;
    assert_eq!(*gc2.borrow(), 2u32, "clone must observe mutation");
}

#[test]
fn nested_gc_value() {
    use oscars_derive::{Finalize, Trace};

    #[derive(Finalize, Trace)]
    struct Wrapper {
        inner: Gc<u64>,
    }

    let nc = NullCollector::default()
        .with_page_size(128)
        .with_heap_threshold(1024);

    let inner = Gc::new_in(42u64, &nc);
    let outer = Gc::new_in(
        Wrapper {
            inner: inner.clone(),
        },
        &nc,
    );

    nc.collect();

    assert_eq!(*inner, 42u64);
    assert_eq!(*outer.inner, 42u64);
}

#[test]
fn weak_gc_always_alive_while_collector_lives() {
    let nc = NullCollector::default();
    let strong = Gc::new_in(10u32, &nc);
    let weak = WeakGc::new_in(&strong, &nc);

    assert!(
        weak.upgrade().is_some(),
        "WeakGc must be upgradeable on a NullCollector"
    );

    drop(strong);
    nc.collect();
}

#[test]
fn weak_map_insert_and_get() {
    let nc = NullCollector::default();
    let key = Gc::new_in(1u64, &nc);
    let mut map = WeakMap::new(&nc);

    map.insert(&key, 100u64, &nc);
    assert_eq!(
        map.get(&key),
        Some(&100u64),
        "WeakMap::get must return the inserted value"
    );
}

#[test]
fn weak_map_drops_cleanly() {
    let nc = NullCollector::default();
    let key = Gc::new_in(7u64, &nc);
    {
        let mut map = WeakMap::new(&nc);
        map.insert(&key, 42u64, &nc);
    }
    nc.collect();
    drop(key);
}

#[test]
fn pools_len_reflects_allocations() {
    let nc = NullCollector::default()
        .with_page_size(64)
        .with_heap_threshold(512);

    assert_eq!(nc.pools_len(), 0, "initially empty");

    let _gc = Gc::new_in(1u64, &nc);
    assert!(nc.pools_len() >= 1, "at least one pool after first alloc");
}

#[test]
fn builder_methods_compile_and_work() {
    let nc = NullCollector::default()
        .with_page_size(4096)
        .with_heap_threshold(8192);

    let gc = Gc::new_in(GcRefCell::new(77u64), &nc);
    nc.collect();
    assert_eq!(*gc.borrow(), 77u64);
}
