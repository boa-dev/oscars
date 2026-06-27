//! mempool4 demo: allocate via CustomPtr, serialize, deserialize, verify
//!
//! what this demo proves:
//! 1. CustomPtr coordinates (pool_id, slot_idx) survive serialization and deserialization
//!    without requiring any pointer fixup passes. A linked list serialized to bytes
//!    can be traversed using the exact same head coordinate after being restored.
//! 2. The allocator's internal state (pool IDs, bump pointers) is correctly restored,
//!    allowing safe incremental allocations after deserialization without colliding
//!    with existing data.
//!
//! Run: `cargo run --example mempool4_demo --features std`

use oscars::alloc::mempool4::{AllocCtx, CustomPtr, Gc, PoolAllocator4, deserialize, serialize};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Entry {
    key: u32,
    value: i64,
    /// Raw CustomPtr of the next entry or 0 for end of list
    next_raw: u32,
}

fn push_front(cx: &AllocCtx<'_>, head_raw: u32, key: u32, value: i64) -> u32 {
    cx.try_alloc(Entry {
        key,
        value,
        next_raw: head_raw,
    })
    .expect("allocation failed")
    .as_custom_ptr()
    .to_raw()
}

fn print_list(cx: &AllocCtx<'_>, head_raw: u32) {
    let mut raw = head_raw;
    while let Some(ptr) = CustomPtr::from_raw(raw) {
        // SAFETY: ptr came from a live allocation or a valid deserialized snapshot.
        let e: &Entry = cx.resolve(unsafe { Gc::from_custom_ptr(ptr) });
        println!("  -> key={} value={}", e.key, e.value);
        raw = e.next_raw;
    }
}

fn collect_list(cx: &AllocCtx<'_>, head_raw: u32) -> Vec<(u32, i64)> {
    let mut out = Vec::new();
    let mut raw = head_raw;
    while let Some(ptr) = CustomPtr::from_raw(raw) {
        let e: &Entry = cx.resolve(unsafe { Gc::from_custom_ptr(ptr) });
        out.push((e.key, e.value));
        raw = e.next_raw;
    }
    out
}

fn main() {
    println!("Phase 1: allocating entries");
    let mut alloc = PoolAllocator4::new().with_page_size(4096);

    let head_raw = alloc.mutate(|cx: AllocCtx<'_>| {
        let mut head = 0u32;
        head = push_front(&cx, head, 30, 3000);
        head = push_front(&cx, head, 20, 2000);
        head = push_front(&cx, head, 10, 1000);
        println!("before serialization:");
        print_list(&cx, head);
        println!(
            "live slots: {}  pool count: {}",
            cx.live_slot_count(),
            cx.pool_count()
        );
        head
    });

    println!("\nPhase 2: Serializing");
    let snapshot = serialize(&alloc);
    println!("snapshot: {} bytes", snapshot.len());

    println!("\nPhase 3: deserializing");
    let mut alloc2 = deserialize(&snapshot).expect("deserialization failed");
    let entries_after = alloc2.mutate(|cx: AllocCtx<'_>| {
        println!("after deserialization:");
        print_list(&cx, head_raw);
        collect_list(&cx, head_raw)
    });

    println!("\nPhase 4: verifying");
    let entries_before = alloc.mutate(|cx: AllocCtx<'_>| collect_list(&cx, head_raw));
    assert_eq!(entries_before, entries_after);
    println!("{} entries match after round trip", entries_before.len());

    println!("\nPhase 5: mutating and re-serializing");
    let new_head = alloc2.mutate(|cx: AllocCtx<'_>| {
        let h = push_front(&cx, head_raw, 5, 500);
        println!("after mutation:");
        print_list(&cx, h);
        h
    });

    let snapshot2 = serialize(&alloc2);
    println!("snapshot 2: {} bytes", snapshot2.len());

    let mut alloc3 = deserialize(&snapshot2).unwrap();
    alloc3.mutate(|cx: AllocCtx<'_>| {
        println!("round trip 2:");
        print_list(&cx, new_head);
    });

    println!("\ndone.");
}
