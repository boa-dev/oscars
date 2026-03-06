use criterion::{criterion_group, criterion_main, Criterion};
use oscars::alloc::arena3::ArenaAllocator;
use std::hint::black_box as bb;

fn bench_arena3_alloc(c: &mut Criterion) {
    let mut group = c.benchmark_group("Arena3 Allocation");

    group.bench_function("Scenario A: Pure Bump Allocation", |b| {
        b.iter_batched(
            || {
                let alloc = ArenaAllocator::default().with_arena_size(1024 * 64);
                alloc
            },
            |mut alloc| {
                for i in 0..1000 {
                    let ptr = alloc.try_alloc(bb(i)).unwrap();
                    bb(ptr);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("Scenario B: Bulk Allocation Loop", |b| {
        let mut alloc = ArenaAllocator::default().with_arena_size(1024 * 1024 * 10);
        b.iter(|| {
            for i in 0..1000 {
                let ptr = alloc.try_alloc(bb(i)).unwrap();
                bb(ptr);
            }
        });
    });

    group.bench_function("Scenario C: Free List Reuse", |b| {
        b.iter_batched(
            || {
                let mut alloc = ArenaAllocator::default().with_arena_size(1024 * 64);
                let mut ptrs = Vec::new();
                for i in 0..1000 {
                    ptrs.push(alloc.try_alloc(i).unwrap());
                }
                for ptr in ptrs {
                    unsafe { alloc.free_slot_typed(ptr.as_ptr()); }
                }
                alloc
            },
            |mut alloc| {
                for i in 0..1000 {
                    let ptr = alloc.try_alloc(bb(i)).unwrap();
                    bb(ptr);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_arena3_alloc);
criterion_main!(benches);
