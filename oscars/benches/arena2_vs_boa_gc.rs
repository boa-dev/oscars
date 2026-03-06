use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use oscars::collectors::mark_sweep_arena2::{
    Finalize, Gc as OscarsGc, MarkSweepGarbageCollector, Trace, TraceColor,
    cell::GcRefCell as OscarsGcRefCell,
};

use boa_gc::{Gc as BoaGc, GcRefCell as BoaGcRefCell, force_collect as boa_force_collect};

fn bench_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc_node_allocation");

    for size in [10, 100, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("arena2", size), size, |b, &size| {
            let collector = MarkSweepGarbageCollector::default()
                .with_arena_size(65536)
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

    for size in [100, 500, 1000].iter() {
        group.bench_with_input(BenchmarkId::new("arena2", size), size, |b, &size| {
            let collector = MarkSweepGarbageCollector::default()
                .with_arena_size(65536)
                .with_heap_threshold(262144);

            b.iter(|| {
                let mut roots = Vec::new();
                for i in 0..size {
                    let root = OscarsGc::new_in(OscarsGcRefCell::new(i), &collector);
                    roots.push(root);
                }
                // let half be garbage
                roots.truncate(size / 2);
                collector.collect();
                black_box(roots.len())
            });
        });

        group.bench_with_input(BenchmarkId::new("boa_gc", size), size, |b, &size| {
            b.iter(|| {
                let mut gcs = Vec::new();
                for i in 0..size {
                    let gc = BoaGc::new(BoaGcRefCell::new(i));
                    gcs.push(gc);
                }
                gcs.truncate(size / 2);
                boa_force_collect();
                black_box(gcs.len())
            });
        });
    }

    group.finish();
}

fn bench_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");

    group.bench_function("arena2", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_arena_size(65536)
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

    group.bench_function("arena2", |b| {
        let collector = MarkSweepGarbageCollector::default()
            .with_arena_size(32768)
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

criterion_group!(
    benches,
    bench_alloc,
    bench_collection,
    bench_mixed,
    bench_pressure,
);

criterion_main!(benches);
