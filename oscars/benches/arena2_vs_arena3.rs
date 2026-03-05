use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn bench_alloc_speed(c: &mut Criterion) {
    let mut group = c.benchmark_group("1_allocation_speed");
    group.significance_level(0.05).sample_size(100);

    for num_objects in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("arena3", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter(|| {
                    let mut allocator =
                        oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(65536);

                    let mut ptrs = Vec::new();
                    for i in 0..num_objects {
                        let ptr = allocator.try_alloc(i).expect("allocation failed");
                        ptrs.push(ptr);
                    }

                    black_box((ptrs.len(), allocator.arenas_len()))
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("arena2", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter(|| {
                    let mut allocator =
                        oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(65536);

                    let mut ptrs = Vec::new();
                    for i in 0..num_objects {
                        let ptr = allocator.try_alloc(i).expect("allocation failed");
                        ptrs.push(ptr);
                    }

                    black_box((ptrs.len(), allocator.arenas_len()))
                });
            },
        );
    }

    group.finish();
}

fn bench_small_objects(c: &mut Criterion) {
    let mut group = c.benchmark_group("2_small_object_overhead");
    group.significance_level(0.05).sample_size(100);

    #[derive(Clone, Copy)]
    struct SmallObject {
        a: u64,
        b: u64,
    }

    for num_objects in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("arena3", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter(|| {
                    let mut allocator =
                        oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(32768);

                    for i in 0..num_objects {
                        let obj = SmallObject {
                            a: i as u64,
                            b: i as u64 * 2,
                        };
                        let _ = allocator.try_alloc(obj).expect("allocation failed");
                    }

                    black_box(allocator.arenas_len())
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("arena2", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter(|| {
                    let mut allocator =
                        oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(32768);

                    for i in 0..num_objects {
                        let obj = SmallObject {
                            a: i as u64,
                            b: i as u64 * 2,
                        };
                        let _ = allocator.try_alloc(obj).expect("allocation failed");
                    }

                    black_box(allocator.arenas_len())
                });
            },
        );
    }

    group.finish();
}

fn bench_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("3_mixed_sizes");

    group.bench_function("arena3", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(65536);

            for _ in 0..50 {
                let _ = allocator.try_alloc([0u8; 16]);
                let _ = allocator.try_alloc([0u8; 32]);
                let _ = allocator.try_alloc([0u8; 64]);
                let _ = allocator.try_alloc([0u8; 128]);
            }

            black_box(allocator.arenas_len())
        });
    });

    group.bench_function("arena2", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(65536);

            for _ in 0..50 {
                let _ = allocator.try_alloc([0u8; 16]);
                let _ = allocator.try_alloc([0u8; 32]);
                let _ = allocator.try_alloc([0u8; 64]);
                let _ = allocator.try_alloc([0u8; 128]);
            }

            black_box(allocator.arenas_len())
        });
    });

    group.finish();
}

// measures how many 16-byte objects fit in a 4KB page before a new one is needed
fn bench_density(c: &mut Criterion) {
    let mut group = c.benchmark_group("4_allocation_density");

    const PAGE_SIZE: usize = 4096;

    group.bench_function("arena3", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(PAGE_SIZE);

            let mut count = 0;
            loop {
                match allocator.try_alloc([0u64; 2]) {
                    Ok(_) => count += 1,
                    Err(_) => break,
                }
                if allocator.arenas_len() > 1 {
                    break;
                }
            }

            black_box((count, allocator.arenas_len()))
        });
    });

    group.bench_function("arena2", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(PAGE_SIZE);

            let mut count = 0;
            loop {
                match allocator.try_alloc([0u64; 2]) {
                    Ok(_) => count += 1,
                    Err(_) => break,
                }
                if allocator.arenas_len() > 1 {
                    break;
                }
            }

            black_box((count, allocator.arenas_len()))
        });
    });

    group.finish();
}

// simulates a Vec doubling from capacity 1 to 1024
fn bench_vec_growth(c: &mut Criterion) {
    let mut group = c.benchmark_group("5_vec_growth");

    group.bench_function("arena3", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(32768);

            let mut cap = 1;
            while cap <= 1024 {
                match cap {
                    1 => {
                        let _ = allocator.try_alloc([0u64; 1]);
                    }
                    2 => {
                        let _ = allocator.try_alloc([0u64; 2]);
                    }
                    4 => {
                        let _ = allocator.try_alloc([0u64; 4]);
                    }
                    8 => {
                        let _ = allocator.try_alloc([0u64; 8]);
                    }
                    16 => {
                        let _ = allocator.try_alloc([0u64; 16]);
                    }
                    32 => {
                        let _ = allocator.try_alloc([0u64; 32]);
                    }
                    64 => {
                        let _ = allocator.try_alloc([0u64; 64]);
                    }
                    128 => {
                        let _ = allocator.try_alloc([0u64; 128]);
                    }
                    256 => {
                        let _ = allocator.try_alloc([0u64; 256]);
                    }
                    512 => {
                        let _ = allocator.try_alloc([0u64; 512]);
                    }
                    1024 => {
                        let _ = allocator.try_alloc([0u64; 1024]);
                    }
                    _ => {}
                }
                cap *= 2;
            }

            black_box(allocator.arenas_len())
        });
    });

    group.bench_function("arena2", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(32768);

            let mut cap = 1;
            while cap <= 1024 {
                match cap {
                    1 => {
                        let _ = allocator.try_alloc([0u64; 1]);
                    }
                    2 => {
                        let _ = allocator.try_alloc([0u64; 2]);
                    }
                    4 => {
                        let _ = allocator.try_alloc([0u64; 4]);
                    }
                    8 => {
                        let _ = allocator.try_alloc([0u64; 8]);
                    }
                    16 => {
                        let _ = allocator.try_alloc([0u64; 16]);
                    }
                    32 => {
                        let _ = allocator.try_alloc([0u64; 32]);
                    }
                    64 => {
                        let _ = allocator.try_alloc([0u64; 64]);
                    }
                    128 => {
                        let _ = allocator.try_alloc([0u64; 128]);
                    }
                    256 => {
                        let _ = allocator.try_alloc([0u64; 256]);
                    }
                    512 => {
                        let _ = allocator.try_alloc([0u64; 512]);
                    }
                    1024 => {
                        let _ = allocator.try_alloc([0u64; 1024]);
                    }
                    _ => {}
                }
                cap *= 2;
            }

            black_box(allocator.arenas_len())
        });
    });

    group.finish();
}

fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("6_sustained_throughput");
    group.throughput(criterion::Throughput::Elements(10000));

    group.bench_function("arena3", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(131072);

            for i in 0..10000 {
                let _ = allocator.try_alloc(i);
            }

            black_box(allocator.arenas_len())
        });
    });

    group.bench_function("arena2", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(131072);

            for i in 0..10000 {
                let _ = allocator.try_alloc(i);
            }

            black_box(allocator.arenas_len())
        });
    });

    group.finish();
}

fn bench_dealloc_speed(c: &mut Criterion) {
    let mut group = c.benchmark_group("7_deallocation_speed");

    //measure the time to free all objects in the arena
    // using `iter_batched` ensures we only measure the deallocation phase
    for num_objects in [100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("arena3", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter_batched(
                    || {
                        let mut allocator =
                            oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(65536);

                        let mut ptrs = Vec::new();
                        for i in 0..num_objects {
                            let ptr = allocator.try_alloc(i).expect("allocation failed");
                            ptrs.push(ptr);
                        }
                        (allocator, ptrs)
                    },
                    |(mut allocator, ptrs)| {
                        for ptr in ptrs {
                            allocator.free_slot(ptr.as_ptr().cast::<u8>());
                        }
                        allocator.drop_dead_arenas();
                        black_box(allocator.arenas_len())
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        group.bench_with_input(
            BenchmarkId::new("arena2", num_objects),
            num_objects,
            |b, &num_objects| {
                b.iter_batched(
                    || {
                        let mut allocator =
                            oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(65536);

                        let mut ptrs = Vec::new();
                        for i in 0..num_objects {
                            let ptr = allocator.try_alloc(i).expect("allocation failed");
                            ptrs.push(ptr);
                        }
                        (allocator, ptrs)
                    },
                    |(mut allocator, ptrs)| {
                        for ptr in ptrs {
                            let mut heap_item_ptr = ptr.as_ptr();
                            unsafe {
                                core::ptr::drop_in_place(heap_item_ptr.as_mut().value_mut());
                                heap_item_ptr.as_mut().mark_dropped();
                            }
                        }
                        allocator.drop_dead_arenas();
                        black_box(allocator.arenas_len())
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_alloc_speed,
    bench_small_objects,
    bench_mixed,
    bench_density,
    bench_vec_growth,
    bench_throughput,
    bench_dealloc_speed,
);

criterion_main!(benches);
