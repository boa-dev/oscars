use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use oscars::{
    GcAllocVec, MarkSweepGarbageCollector, Root as OscarsRoot, cell::GcRefCell as OscarsGcRefCell,
};

use boa_gc::{Gc as BoaGc, GcRefCell as BoaGcRefCell, force_collect as boa_force_collect};

// benchmark: create N simple gc nodes
fn bench_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc_node_allocation");

    for size in [10, 100, 1000].iter() {
        // oscars
        group.bench_with_input(BenchmarkId::new("oscars", size), size, |b, &size| {
            let collector = MarkSweepGarbageCollector::default()
                .with_arena_size(65536)
                .with_heap_threshold(262144);

            b.iter(|| {
                let mut roots = Vec::new();
                for i in 0..size {
                    let root = OscarsRoot::new_in(OscarsGcRefCell::new(i), &collector);
                    roots.push(root);
                }
                black_box(roots.len())
            });
        });

        // boa_gc
        group.bench_with_input(BenchmarkId::new("boa_gc", size), size, |b, &size| {
            b.iter_batched(
                || {
                    boa_force_collect();
                }, // drain previous iteration's garbage
                |()| {
                    let mut gcs = Vec::new();
                    for i in 0..size {
                        let gc = BoaGc::new(BoaGcRefCell::new(i));
                        gcs.push(gc);
                    }
                    black_box(gcs.len())
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// benchmark: gc collection performance with live objects
fn bench_collection(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc_collection_pause");

    for num_objects in [100, 500, 1000].iter() {
        // oscars implementation
        group.bench_with_input(
            BenchmarkId::new("oscars", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter_batched(
                    || {
                        //create collector with many live objects
                        let collector = MarkSweepGarbageCollector::default()
                            .with_arena_size(65536)
                            .with_heap_threshold(262144);
                        let mut roots = Vec::new();
                        for i in 0..num_objects {
                            let root = OscarsRoot::new_in(OscarsGcRefCell::new(i), &collector);
                            roots.push(root);
                        }
                        (collector, roots)
                    },
                    |(collector, roots)| {
                        collector.collect();
                        black_box(roots.len())
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        // boa_gc implementation
        group.bench_with_input(
            BenchmarkId::new("boa_gc", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter_batched(
                    || {
                        // drain any garbage left over from the previous batch so
                        // the global boa_gc heap holds exactly num_objects live
                        // objects when the timed routine runs.
                        boa_force_collect();
                        let mut gcs = Vec::new();
                        for i in 0..num_objects {
                            let gc = BoaGc::new(BoaGcRefCell::new(i));
                            gcs.push(gc);
                        }
                        gcs
                    },
                    |gcs| {
                        boa_force_collect();
                        black_box(gcs.len())
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// benchmark: creating vectors - oscars with gc allocator vs boa_gc with system allocator
fn bench_vec_create(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_creation");

    for vec_size in [10, 100, 1000].iter() {
        // oscars with GC allocator
        group.bench_with_input(
            BenchmarkId::new("oscars_gc_allocator", vec_size),
            vec_size,
            |b, &vec_size| {
                let collector = MarkSweepGarbageCollector::default()
                    .with_arena_size(65536)
                    .with_heap_threshold(262144);

                b.iter(|| {
                    let vec = GcAllocVec::with_capacity(vec_size, &collector);
                    let gc_vec = OscarsRoot::new_in(OscarsGcRefCell::new(vec), &collector);

                    for i in 0..vec_size {
                        gc_vec.borrow_mut().push(black_box(i));
                    }

                    black_box(gc_vec.borrow().len())
                });
            },
        );

        // boa_gc with standard Vec
        // wraps first then pushes through borrow_mut so both sides pay the same
        // per element GcRefCell borrow cost, making the comparison fair
        group.bench_with_input(
            BenchmarkId::new("boa_gc_std_vec", vec_size),
            vec_size,
            |b, &vec_size| {
                b.iter(|| {
                    let gc_vec = BoaGc::new(BoaGcRefCell::new(Vec::with_capacity(vec_size)));
                    for i in 0..vec_size {
                        gc_vec.borrow_mut().push(black_box(i));
                    }
                    black_box(gc_vec.borrow().len())
                });
            },
        );
    }

    group.finish();
}

// benchmark: vec operations with gc pointers inside
fn bench_vec_ptrs(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_of_gc_pointers");

    for num_elements in [10, 50, 100].iter() {
        // oscars: GcAllocVec containing Gc pointers
        group.bench_with_input(
            BenchmarkId::new("oscars_gc_vec_gc_ptrs", num_elements),
            num_elements,
            |b, &num_elements| {
                let collector = MarkSweepGarbageCollector::default()
                    .with_arena_size(65536)
                    .with_heap_threshold(262144);

                b.iter(|| {
                    let vec = GcAllocVec::with_capacity(num_elements, &collector);
                    let gc_vec = OscarsRoot::new_in(OscarsGcRefCell::new(vec), &collector);

                    // create gc pointers and add to vec
                    for i in 0..num_elements {
                        let inner = OscarsRoot::new_in(OscarsGcRefCell::new(i), &collector);
                        gc_vec.borrow_mut().push(inner.into_gc());
                    }

                    // sum to ensure access
                    let sum: usize = gc_vec.borrow().iter().map(|gc| *gc.borrow()).sum();

                    black_box(sum)
                });
            },
        );

        // boa_gc: std::Vec containing Gc pointers
        group.bench_with_input(
            BenchmarkId::new("boa_gc_std_vec_gc_ptrs", num_elements),
            num_elements,
            |b, &num_elements| {
                b.iter(|| {
                    let mut vec = Vec::with_capacity(num_elements);

                    // create gc pointers and add to vec
                    for i in 0..num_elements {
                        let gc = BoaGc::new(BoaGcRefCell::new(i));
                        vec.push(gc);
                    }

                    let gc_vec = BoaGc::new(BoaGcRefCell::new(vec));

                    let sum: usize = gc_vec.borrow().iter().map(|gc| *gc.borrow()).sum();

                    black_box(sum)
                });
            },
        );
    }

    group.finish();
}

// benchmark: mixed workload - allocations + collections
fn bench_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");

    // oscars: allocate, collect, allocate more, collect again
    group.bench_function("oscars_alloc_collect_cycle", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_arena_size(65536)
            .with_heap_threshold(131072);

        b.iter(|| {
            let mut roots = Vec::new();

            for i in 0..100 {
                let root = OscarsRoot::new_in(OscarsGcRefCell::new(i), &collector);
                roots.push(root);
            }

            collector.collect();

            for i in 100..200 {
                let root = OscarsRoot::new_in(OscarsGcRefCell::new(i), &collector);
                roots.push(root);
            }
            collector.collect();

            black_box(roots.len())
        });
    });

    // boa_gc: same pattern
    group.bench_function("boa_gc_alloc_collect_cycle", |b| {
        b.iter(|| {
            let mut gcs = Vec::new();

            for i in 0..100 {
                let gc = BoaGc::new(BoaGcRefCell::new(i));
                gcs.push(gc);
            }

            boa_force_collect();

            for i in 100..200 {
                let gc = BoaGc::new(BoaGcRefCell::new(i));
                gcs.push(gc);
            }

            boa_force_collect();

            black_box(gcs.len())
        });
    });

    group.finish();
}

// benchmark: memory pressure scenario
fn bench_pressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_pressure");

    // oscars
    group.bench_function("oscars_churn", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_arena_size(32768)
            .with_heap_threshold(65536);

        b.iter(|| {
            let mut live_set = Vec::new();

            for round in 0..10 {
                // allocate temporary objects
                for i in 0..50 {
                    let temp =
                        OscarsRoot::new_in(OscarsGcRefCell::new(round * 100 + i), &collector);
                    // only keep every 10th object
                    if i % 10 == 0 {
                        live_set.push(temp);
                    }
                }

                collector.collect();
            }

            black_box(live_set.len())
        });
    });

    // boa_gc
    group.bench_function("boa_gc_churn", |b| {
        b.iter(|| {
            let mut live_set = Vec::new();

            for round in 0..10 {
                // allocate temporary objects
                for i in 0..50 {
                    let temp = BoaGc::new(BoaGcRefCell::new(round * 100 + i));
                    // only keep every 10th object
                    if i % 10 == 0 {
                        live_set.push(temp);
                    }
                }

                boa_force_collect();
            }

            black_box(live_set.len())
        });
    });

    group.finish();
}

// benchmark: deep object graphs
fn bench_deep(c: &mut Criterion) {
    let mut group = c.benchmark_group("deep_object_graph");

    #[derive(Clone)]
    struct Node {
        value: usize,
        children: Vec<oscars::Gc<OscarsGcRefCell<Node>>>,
    }

    impl oscars::Finalize for Node {}

    // SAFETY: we trace all children, making them visible to the collector
    unsafe impl oscars::Trace for Node {
        unsafe fn trace(&self, color: oscars::TraceColor) {
            for child in &self.children {
                // SAFETY: called during mark phase only
                unsafe { child.trace(color) };
            }
        }

        fn run_finalizer(&self) {
            oscars::Finalize::finalize(self);
        }
    }

    // oscars: create tree with branching factor 3, depth 5
    group.bench_function("oscars_tree_depth_5", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_arena_size(131072)
            .with_heap_threshold(262144);

        b.iter(|| {
            fn create_tree(
                depth: usize,
                collector: &MarkSweepGarbageCollector,
            ) -> oscars::Gc<OscarsGcRefCell<Node>> {
                let node = if depth == 0 {
                    Node {
                        value: depth,
                        children: Vec::new(),
                    }
                } else {
                    let mut children = Vec::new();
                    for _ in 0..3 {
                        let child_gc = create_tree(depth - 1, collector);
                        children.push(child_gc);
                    }
                    Node {
                        value: depth,
                        children,
                    }
                };

                let root = OscarsRoot::new_in(OscarsGcRefCell::new(node), collector);
                root.into_gc()
            }

            let tree_root = create_tree(5, &collector);
            collector.collect();
            black_box(tree_root.borrow().value)
        });
    });

    #[derive(Clone, boa_gc::Trace, boa_gc::Finalize)]
    struct BoaNode {
        value: usize,
        children: Vec<BoaGc<BoaGcRefCell<BoaNode>>>,
    }

    // boa_gc: same tree structure
    group.bench_function("boa_gc_tree_depth_5", |b| {
        b.iter(|| {
            fn create_tree(depth: usize) -> BoaGc<BoaGcRefCell<BoaNode>> {
                let node = if depth == 0 {
                    BoaNode {
                        value: depth,
                        children: Vec::new(),
                    }
                } else {
                    let mut children = Vec::new();
                    for _ in 0..3 {
                        children.push(create_tree(depth - 1));
                    }
                    BoaNode {
                        value: depth,
                        children,
                    }
                };

                BoaGc::new(BoaGcRefCell::new(node))
            }

            let tree = create_tree(5);
            boa_force_collect();
            black_box(tree.borrow().value)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_alloc,
    bench_collection,
    bench_vec_create,
    bench_vec_ptrs,
    bench_mixed,
    bench_pressure,
    bench_deep,
);

criterion_main!(benches);
