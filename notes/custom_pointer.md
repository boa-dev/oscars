# Research Notes: Custom Pointers for Oscars

This document answers the questions from issue #86 about adding custom pointers to oscars

Our main goal is to find a reliable way to point to memory on the heap. Unlike normal pointers, we want these pointers to work even if the operating system loads the program at a different memory address next time. This makes it possible to "pin" specific objects and easily serialize/deserialize the heap.

We can build a custom pointer by combining our `MempoolAllocator` design with ideas from modern GC research.

Reference: https://kyju.org/blog/tokioconf-2026/#a-sketch-of-a-real-raw-pointer-based-gc

## 1. What is the most optimal representation for that pointer?

To be able to serialize and deserialize the heap, we cannot use regular memory addresses (`*mut T` or `NonNull<T>`). Regular addresses change every time we run the program. Instead, we need a stable ID.

Since we use a `MempoolAllocator`, which organizes memory into blocks called pools, the best choice is a **Segmented ID**

### Segmented ID Representation
A custom pointer should just be a 32 bit number, `u32` or `NonZeroU32` so `Option<CustomPtr>` stays small

This 32 bit number is split into two parts:
- **`pool_id`:** Tells us which pool the object is in.
- **`slot_idx`:** Tells us the exact slot within that pool.

**Why this is the best choice:**
1. **Fits Mempool Perfectly:** `MempoolAllocator` already organizes memory into Pools and Slots, this ID directly matches that setup.
2. **Saves Memory:** Using a 32 bit number instead of a 64 bit pointer cuts the size of all GC references in half, making the program faster because more data fits in the CPU cache.
3. **Easy to Serialize/Deserialize:** The ID is just a logical coordinate (`pool_id`, `slot_idx`), not a physical memory address. When we deserialize a serialized heap, these coordinates still point to the correct objects, even if the OS puts the pools in a different physical location.

## 2. What is the API for a custom pointer?

Because a custom pointer is just an index and not a real pointer, so it cannot safely use the `core::ops::Deref` trait. we can't turn a number into a reference without knowing where the memory is actually stored.

Instead, we use branding, i.e. we wrap the 32 bit number in type `Gc<'gc, T>`

### The `Gc` Wrapper
```rust
use core::num::NonZeroU32;
use core::marker::PhantomData;

/// 32 bit number
#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct CustomPtr(NonZeroU32);

/// GC pointer
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct Gc<'gc, T: ?Sized> {
    ptr: CustomPtr,
    _marker: PhantomData<(&'gc (), *const T)>, 
}
```

### The `Deref` Problem
Right now, the new `mark_sweep_branded` and `null_collector_branded` APIs wrap real physical pointers (`NonNull`) and implement `Deref`. This makes them easy to use.

If we change our pointers to be 32 bit Segmented IDs, **we will lose the ability to use `Deref`**. 

Instead, developers will have to pass the pointer back to the GC context (`MutationContext` or `ArenaCtx`) to read the data:

```rust
impl<'gc> MutationContext<'gc> {
    /// Turns the custom pointer into a real Rust ref
    pub fn get<T: Trace>(&self, gc: Gc<'gc, T>) -> &T {
        // Looks up the memory using the pool_id and slot_idx
    }
}
```

If we really want to keep `Deref` for ease of use, there are two workarounds:
1. **Thread-Local Storage (TLS):** Put the `MempoolAllocator` in a `thread_local!` variable. This lets `Deref` look up the memory secretly behind the scenes. This is easy to use but makes it harder to move the heap between threads.
2. **Hybrid Approach:** Keep using real pointers (`NonNull`) for `Gc` during normal code execution so `Deref` works. But, create a new `HeapPtr` type that uses the Segmented ID only when we need to pin or serialize the object to disk.

### Benefits of Thread safety
A huge benefit of using a 32 bit index is that it is totally harmless on its own. we can't read the memory without the `MutationContext`.

Because of this, `Gc<'gc, T>` can safely implement `Send` and `Sync`. We can safely pass these pointers between different threads, even if the data they point to can be mutated, like `Cell`

```rust
// Safe because Gc is just 32 bit number
unsafe impl<'gc, T> Send for Gc<'gc, T> {}
unsafe impl<'gc, T> Sync for Gc<'gc, T> {}
```

## 3. How should memory stores and loads work?

To read or write memory, we must always use the context that owns the `MempoolAllocator`.

### Looking up the Memory
When we call `ctx.get(gc)` or `ctx.get_mut(gc)`, the context breaks the 32 bit number into its two parts and finds the memory:

```rust
impl CustomPtr {
    #[inline(always)]
    pub fn pool_id(&self) -> usize {
        (self.0.get() >> 20) as usize // top 12 bits
    }

    #[inline(always)]
    pub fn slot_idx(&self) -> usize {
        (self.0.get() & 0x000F_FFFF) as usize // bottom 20 bits
    }
}

// Inside MutationContext::get:
let pool_id = gc.ptr.pool_id();
let slot_idx = gc.ptr.slot_idx();

// find the pool
let pool = &self.heap.pools[pool_id];

// find the exact slot
let value_ref = pool.get_slot(slot_idx);
```

### Performance
Even though this requires two lookups, finding the pool and then finding the slot, it is extremely fast. The table of pools is small and stays in the CPU cache. The speed gained from using 32 bit pointers more than makes up for this tiny delay.

### Serializing and Deserializing the Heap
With this design, serializing the heap to disk is very easy:

1. **Pause Changes:** Make sure no code is currently modifying the heap.
2. **Serialize Pools:** Loop through all the pools in the `MempoolAllocator`. Serialize their metadata (like the ID) and write the raw bytes of all used slots to disk.
3. **Serialize Roots:** Serialize the 32 bit IDs of any root objects.
4. **Deserialize the Heap:** Recreate the `MempoolAllocator` with the exact same pool IDs and deserialize the raw bytes back in. Because all Gc pointers are just `(pool_id, slot_idx)` numbers, they will automatically point to the right places. We do not need to rewrite or fix any pointers
