# arena2 vs arena3 benchmark results

date: 2026-03-02
Note author: shruti2522

this goes over the results of the `arena2_vs_arena3` bench suite. The question was
whether arena3's size class bitmap design is worth using over arena2's simpler
linked list with per slot headers. 
answer: arena2 is faster at raw allocation,
but arena3 fits more objects into the same amount of memory

## Results

### 1. Allocation speed

arena2 is faster at every size, and the gap grows with object count.

| objects | arena3  | arena2  |
| 100     | 1.02 µs | 643 ns  |
| 500     | 4.15 µs | 1.83 µs |
| 1000    | 8.36 µs | 2.77 µs |

At 100 objects arena2 is roughly 1.6x faster, at 1000 it is 3x. arena3 is slower
for two reasons: it has to do a size class lookup on every allocation (finding the
right arena for the object's size) and set a bit in the bitmap. arena2 just moves
a pointer forward and writes an 8 byte header

### 2. Small object overhead

arena2 is faster here too, which might seem odd given that it writes an 8 byte
header for every object. But this bench measures allocation time, not memory use.
writing the header is cheap, what costs time in arena3 is the size class routing.

| objects | arena3 (0-byte header) | arena2 (8-byte header) |
| 100     | 781 ns                 | 257 ns                 |
| 500     | 3.56 µs                | 1.08 µs                |
| 1000    | 7.02 µs                | 2.15 µs                |

arena2 is roughly 3.3x faster across all sizes here, the cost of the bitmap and
size class lookup shows up clearly when the objects are small.

### 3. Size class fragmentation (mixed sizes)

Allocating objects of four different sizes (16, 32, 64, 128 bytes) in interleaved
batches of 50 each:

- arena3: 1.878 µs  
- arena2: 441 ns

arena2 is ~4.3x faster, arena3 sends each allocation to a different per size arena,
which means more branching and more work keeping track of arena pointers.

### 4. Memory efficiency

How many 16 byte objects fit in a single 4KB arena before it requests a new one:

- arena3: **254 objects**
- arena2: **170 objects**

arena3 fits ~49% more objects per page. The reason is the 8 byte header that
arena2 adds to every slot. A 1 -byte object takes 24 bytes under arena2. arena3
tracks liveness in a bitmap stored at the top of the page instead, so each slot
stays 16 bytes

this is the number that drove the decision, Fewer pages means fewer pointer
reads during the sweep phase, better cache use, and less work for the collector.

### 5. Vec growth pattern

Simulating a Vec doubling from capacity 1 to 1024 (11 allocations of increasing
size):

- arena3: 1.12 µs  
- arena2: 370 ns

arena2 is ~3x faster. For a growing vec the size class lookup cost hits on every
doubling step because each new size lands in a different arena.

### 6. Sustained throughput (10k allocations)

- arena3: 71.5 µs  
- arena2: 23.0 µs

arena2 is ~3.1x faster at a steady allocation rate, this is the biggest gap in
the whole suite

## What this means

arena2 wins every timing number in this suite. But for a GC, allocation is only half the work, the other half is how
cheap it is to sweep dead objects and how well the heap fits in the cache.

254 vs 170 objects per 4KB page means fewer pages to walk and less memory for the
mark phase to read. arena2 also requires reading and decoding an 8 byte header on
every slot during the sweep. arena3's bitmap checks 64 slots at once with a single
64 bit word read and a `trailing_zeros` call

The trad off is on purpose. arena3 pays more at allocation time to get cheaper
collection, a smaller heap, and better cache behavior during the sweep. The
supertrait benchmark results confirm this holds in practice. The collection pause
improvements against boa_gc come from arena3's sweep being cheaper

## Things to consider

- the allocation slowdown matters for workloads that alloc a lot and collect rarely.
  Worth profiling Boa's JS workloads to check the alloc/collect ratio.
- the size class lookup at mixed sizes is the main cost, a binary search or a small
  table indexed by leading zeros could speed it up without changing the bitmap
