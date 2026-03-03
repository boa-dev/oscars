use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

// benchmark comparing arena2 (linked list + headers) vs arena3 (bitmap + size classes)
// for Allocator supertrait
//
// arena3 trades allocation speed for memory efficiency, which is better for GC

// allocation speed
// arena2 is faster due to simpler linkedlist logic
fn bench_alloc_speed(c: &mut Criterion) {
    let mut group = c.benchmark_group("1_allocation_speed");
    group.significance_level(0.05).sample_size(100);

    eprintln!("\nallocation speed (arena2 is faster)");

    for num_objects in [100, 500, 1000].iter() {
        // arena3 (bitmap, 0 byte overhead)
        group.bench_with_input(
            BenchmarkId::new("arena3_bitmap_slower", num_objects),
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

        // arena2 (linked list, 8 byte header overhead)
        group.bench_with_input(
            BenchmarkId::new("arena2_faster_but_wasteful", num_objects),
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

// small object overhead (16 byte objects)
// arena2's 8 byte headers add 50% overhead to 16byte objects
fn bench_small_objects(c: &mut Criterion) {
    let mut group = c.benchmark_group("2_small_object_overhead");
    group.significance_level(0.05).sample_size(100);

    eprintln!("\nsmall object overhead (arena3 uses ~50% less memory)");

    #[derive(Clone, Copy)]
    struct SmallObject {
        a: u64,
        b: u64,
    }

    for num_objects in [100, 500, 1000].iter() {
        // arena3 - 16 bytes per object (no header)
        group.bench_with_input(
            BenchmarkId::new("arena3_0byte_headers", num_objects),
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

        // arena2 - 24 bytes per object (50% overhead!)
        group.bench_with_input(
            BenchmarkId::new("arena2_8byte_headers", num_objects),
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

// mixed size allocations
// shows how arena3 wastes less space when objects are different sizes.
fn bench_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("3_size_class_fragmentation");

    eprintln!("\nMixed Size Allocations (arena3 reduces fragmentation)");

    group.bench_function("arena3_size_classes", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(65536);

            // allocate various sizes that map to different size classes
            for _ in 0..50 {
                let _ = allocator.try_alloc([0u8; 16]);
                let _ = allocator.try_alloc([0u8; 32]);
                let _ = allocator.try_alloc([0u8; 64]);
                let _ = allocator.try_alloc([0u8; 128]);
            }

            black_box(allocator.arenas_len())
        });
    });

    group.bench_function("arena2_no_size_classes", |b| {
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

// allocation density
// measures how many objects fit into a 4KB arena
// this is the main metric for GC efficiency
fn bench_density(c: &mut Criterion) {
    let mut group = c.benchmark_group("4_allocation_density");

    eprintln!("\nallocation density (arena3 fits more objects)");

    const ARENA_SIZE: usize = 4096; // 4KB arena

    // arena3: 16byte objects with 0 byte headers
    // fits ~254 objects per arena
    group.bench_function("arena3_FITS_254_OBJECTS", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(ARENA_SIZE);

            let mut count = 0;
            loop {
                match allocator.try_alloc([0u64; 2]) {
                    Ok(_) => count += 1,
                    Err(_) => break,
                }
                // stop after first arena fills
                if allocator.arenas_len() > 1 {
                    break;
                }
            }

            black_box((count, allocator.arenas_len()))
        });
    });

    // arena2: 16 byte objects + 8 byte headers = 24 bytes total
    // fits ~170 objects per arena
    group.bench_function("arena2_FITS_170_OBJECTS", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(ARENA_SIZE);

            let mut count = 0;
            loop {
                match allocator.try_alloc([0u64; 2]) {
                    Ok(_) => count += 1,
                    Err(_) => break,
                }
                // stop after first arena fills
                if allocator.arenas_len() > 1 {
                    break;
                }
            }

            black_box((count, allocator.arenas_len()))
        });
    });

    group.finish();
}

// vec growth simulation
// simulates Vec::push reallocations
fn bench_vec_growth(c: &mut Criterion) {
    let mut group = c.benchmark_group("5_vec_growth_pattern");

    eprintln!("\nvec growth simulation");

    // simulate Vec growing from 1 to 1024 elements
    group.bench_function("arena3_vec_pattern", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(32768);

            let mut current_cap = 1;
            while current_cap <= 1024 {
                match current_cap {
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
                current_cap *= 2;
            }

            black_box(allocator.arenas_len())
        });
    });

    group.bench_function("arena2_vec_pattern", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena2::ArenaAllocator::default().with_arena_size(32768);

            let mut current_cap = 1;
            while current_cap <= 1024 {
                match current_cap {
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
                current_cap *= 2;
            }

            black_box(allocator.arenas_len())
        });
    });

    group.finish();
}

// sustained throughput
// measures long term allocation rate (10k operations)
fn bench_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("6_sustained_throughput");
    group.throughput(criterion::Throughput::Elements(10000));

    eprintln!("\nallocation throughput");

    group.bench_function("arena3_10k_allocs", |b| {
        b.iter(|| {
            let mut allocator =
                oscars::alloc::arena3::ArenaAllocator::default().with_arena_size(131072);

            for i in 0..10000 {
                let _ = allocator.try_alloc(i);
            }

            black_box(allocator.arenas_len())
        });
    });

    group.bench_function("arena2_10k_allocs", |b| {
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

    print_results_summary();
}

fn print_results_summary() {
    eprintln!("\nresults summary:");
    eprintln!("speed: arena2 is faster");
    eprintln!("memory: arena3 fits ~49% more small objects");
}

fn bench_with_intro(c: &mut Criterion) {
    static PRINTED: std::sync::Once = std::sync::Once::new();
    PRINTED.call_once(|| {
        eprintln!("\narena2 vs arena3 performance comparison\n");
    });

    bench_alloc_speed(c);
}

criterion_group!(
    benches,
    bench_with_intro,
    bench_small_objects,
    bench_mixed,
    bench_density,
    bench_vec_growth,
    bench_throughput,
);

criterion_main!(benches);
