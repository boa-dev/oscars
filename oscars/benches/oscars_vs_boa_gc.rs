use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use oscars::mark_sweep::{
    Gc as OscarsGc, MarkSweepGarbageCollector, cell::GcRefCell as OscarsGcRefCell,
};

use boa_gc::{Gc as BoaGc, GcRefCell as BoaGcRefCell, force_collect as boa_force_collect};

fn bench_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc_node_allocation");

    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("oscars", size), size, |b, &size| {
            let collector = MarkSweepGarbageCollector::default()
                .with_page_size(65536)
                .with_heap_threshold(262144);

            b.iter(|| {
                let mut roots = Vec::new();
                for i in 0..size {
                    let root = OscarsGc::new_in(OscarsGcRefCell::new(i), &collector);
                    roots.push(root);
                }
                black_box(roots.len())
            });
        });

        group.bench_with_input(BenchmarkId::new("boa_gc", size), size, |b, &size| {
            b.iter_batched(
                || {
                    boa_force_collect();
                },
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

fn bench_collection(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc_collection_pause");

    for num_objects in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("oscars", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter_batched(
                    || {
                        let collector = MarkSweepGarbageCollector::default()
                            .with_page_size(65536)
                            .with_heap_threshold(262144);
                        let mut roots = Vec::new();
                        for i in 0..num_objects {
                            let root = OscarsGc::new_in(OscarsGcRefCell::new(i), &collector);
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

        group.bench_with_input(
            BenchmarkId::new("boa_gc", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter_batched(
                    || {
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

fn bench_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");

    group.bench_function("oscars", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_page_size(65536)
            .with_heap_threshold(131072);

        b.iter(|| {
            let mut roots = Vec::new();

            for i in 0..100 {
                let root = OscarsGc::new_in(OscarsGcRefCell::new(i), &collector);
                roots.push(root);
            }
            collector.collect();

            for i in 100..200 {
                let root = OscarsGc::new_in(OscarsGcRefCell::new(i), &collector);
                roots.push(root);
            }
            collector.collect();

            black_box(roots.len())
        });
    });

    group.bench_function("boa_gc", |b| {
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

fn bench_pressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_pressure");

    group.bench_function("oscars", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_page_size(32768)
            .with_heap_threshold(65536);

        b.iter(|| {
            let mut live = Vec::new();

            for round in 0..10 {
                for i in 0..50 {
                    let obj = OscarsGc::new_in(OscarsGcRefCell::new(round * 100 + i), &collector);
                    if i % 10 == 0 {
                        live.push(obj);
                    }
                }
                collector.collect();
            }

            black_box(live.len())
        });
    });

    group.bench_function("boa_gc", |b| {
        b.iter(|| {
            let mut live = Vec::new();

            for round in 0..10 {
                for i in 0..50 {
                    let obj = BoaGc::new(BoaGcRefCell::new(round * 100 + i));
                    if i % 10 == 0 {
                        live.push(obj);
                    }
                }
                boa_force_collect();
            }

            black_box(live.len())
        });
    });

    group.finish();
}

fn bench_deep(c: &mut Criterion) {
    let mut group = c.benchmark_group("deep_object_graph");

    #[derive(Clone)]
    struct Node {
        value: usize,
        children: Vec<OscarsGc<OscarsGcRefCell<Node>>>,
    }

    impl oscars::mark_sweep::Finalize for Node {}

    // SAFETY: we trace all children, making them visible to the collector
    unsafe impl oscars::mark_sweep::Trace for Node {
        unsafe fn trace(&self, color: oscars::mark_sweep::TraceColor) {
            for child in &self.children {
                unsafe { child.trace(color) };
            }
        }

        fn run_finalizer(&self) {
            oscars::mark_sweep::Finalize::finalize(self);
        }
    }

    group.bench_function("oscars", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_page_size(131072)
            .with_heap_threshold(262144);

        b.iter(|| {
            fn build_tree(
                depth: usize,
                collector: &MarkSweepGarbageCollector,
            ) -> OscarsGc<OscarsGcRefCell<Node>> {
                let node = if depth == 0 {
                    Node {
                        value: depth,
                        children: Vec::new(),
                    }
                } else {
                    let children = (0..3).map(|_| build_tree(depth - 1, collector)).collect();
                    Node {
                        value: depth,
                        children,
                    }
                };
                OscarsGc::new_in(OscarsGcRefCell::new(node), collector)
            }

            let root = build_tree(5, &collector);
            collector.collect();
            black_box(root.borrow().value)
        });
    });

    #[derive(Clone, boa_gc::Trace, boa_gc::Finalize)]
    struct BoaNode {
        value: usize,
        children: Vec<BoaGc<BoaGcRefCell<BoaNode>>>,
    }

    group.bench_function("boa_gc", |b| {
        b.iter(|| {
            fn build_tree(depth: usize) -> BoaGc<BoaGcRefCell<BoaNode>> {
                let node = if depth == 0 {
                    BoaNode {
                        value: depth,
                        children: Vec::new(),
                    }
                } else {
                    let children = (0..3).map(|_| build_tree(depth - 1)).collect();
                    BoaNode {
                        value: depth,
                        children,
                    }
                };
                BoaGc::new(BoaGcRefCell::new(node))
            }

            let root = build_tree(5);
            boa_force_collect();
            black_box(root.borrow().value)
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_alloc,
    bench_collection,
    bench_mixed,
    bench_pressure,
    bench_deep,
);

criterion_main!(benches);
