use super::{AllocCtx, CustomPtr, Gc, PoolAllocator4, deserialize, serialize};

#[test]
fn alloc_and_resolve() {
    let mut alloc = PoolAllocator4::new();
    alloc.mutate(|cx: AllocCtx<'_>| {
        let a: Gc<'_, i32> = cx.try_alloc(42_i32).unwrap();
        let b: Gc<'_, i32> = cx.try_alloc(-7_i32).unwrap();
        let c: Gc<'_, u64> = cx.try_alloc(u64::MAX).unwrap();
        assert_eq!(*cx.resolve(a), 42);
        assert_eq!(*cx.resolve(b), -7);
        assert_eq!(*cx.resolve(c), u64::MAX);
    });
}

#[test]
fn custom_ptr_encoding() {
    use super::ptr::{MAX_POOL_ID, MAX_SLOT_IDX};

    let cases: &[(u32, u32)] = &[
        (0, 0),
        (0, 1),
        (1, 0),
        (42, 1_000),
        (4095, 0),
        (0, 1_048_575),
    ];
    for &(pid, sidx) in cases {
        let ptr = CustomPtr::new(pid, sidx)
            .unwrap_or_else(|| panic!("CustomPtr::new({pid}, {sidx}) returned None"));
        assert_eq!(ptr.pool_id() as u32, pid);
        assert_eq!(ptr.slot_idx() as u32, sidx);
    }
    assert!(CustomPtr::new(MAX_POOL_ID + 1, 0).is_none());
    assert!(CustomPtr::new(0, MAX_SLOT_IDX + 1).is_none());
    assert!(CustomPtr::new(MAX_POOL_ID, MAX_SLOT_IDX).is_none());
}

#[test]
fn free_and_reuse() {
    let mut alloc = PoolAllocator4::new();
    alloc.mutate(|cx: AllocCtx<'_>| {
        let gc1: Gc<'_, u64> = cx.try_alloc(111_u64).unwrap();
        let slot_before = gc1.as_custom_ptr().slot_idx();
        let pool_before = gc1.as_custom_ptr().pool_id();

        // gc1 is not used after this call
        unsafe { cx.free(gc1) };

        let gc2: Gc<'_, u64> = cx.try_alloc(222_u64).unwrap();
        assert_eq!(gc2.as_custom_ptr().pool_id(), pool_before);
        assert_eq!(gc2.as_custom_ptr().slot_idx(), slot_before);
        assert_eq!(*cx.resolve(gc2), 222);
    });
}

// Gc must be Send + Sync
fn _assert_send_sync<T: Send + Sync>() {}
fn _check_gc_send_sync() {
    _assert_send_sync::<Gc<'static, i32>>();
}

#[test]
fn serialize_roundtrip() {
    let (bytes, a_ptr, b_ptr, c_ptr) = {
        let mut alloc = PoolAllocator4::new();
        let ptrs = alloc.mutate(|cx: AllocCtx<'_>| {
            let a = cx.try_alloc(100_u32).unwrap();
            let b = cx.try_alloc(200_u32).unwrap();
            let c = cx.try_alloc(300_u32).unwrap();
            (a.as_custom_ptr(), b.as_custom_ptr(), c.as_custom_ptr())
        });
        (serialize(&alloc), ptrs.0, ptrs.1, ptrs.2)
    };

    assert!(!bytes.is_empty());
    let mut alloc2 = deserialize(&bytes).unwrap();
    alloc2.mutate(|cx: AllocCtx<'_>| {
        // coordinates came from live slots
        let a2: Gc<'_, u32> = unsafe { Gc::from_custom_ptr(a_ptr) };
        let b2: Gc<'_, u32> = unsafe { Gc::from_custom_ptr(b_ptr) };
        let c2: Gc<'_, u32> = unsafe { Gc::from_custom_ptr(c_ptr) };
        assert_eq!(*cx.resolve(a2), 100);
        assert_eq!(*cx.resolve(b2), 200);
        assert_eq!(*cx.resolve(c2), 300);
    });
}

#[test]
fn serialize_roots() {
    /// Node storing value and next CustomPtr
    #[derive(Copy, Clone)]
    struct Node {
        value: u32,
        next_raw: u32,
    }

    let (bytes, head_raw) = {
        let mut alloc = PoolAllocator4::new();
        let head_raw = alloc.mutate(|cx: AllocCtx<'_>| {
            let tail = cx
                .try_alloc(Node {
                    value: 99,
                    next_raw: 0,
                })
                .unwrap();
            let head = cx
                .try_alloc(Node {
                    value: 1,
                    next_raw: tail.as_custom_ptr().to_raw(),
                })
                .unwrap();
            head.as_custom_ptr().to_raw()
        });
        (serialize(&alloc), head_raw)
    };

    let mut alloc2 = deserialize(&bytes).unwrap();
    alloc2.mutate(|cx: AllocCtx<'_>| {
        // head_raw was serialized from a live allocation
        let root: Gc<'_, Node> =
            unsafe { Gc::from_custom_ptr(CustomPtr::from_raw(head_raw).unwrap()) };
        let head = *cx.resolve(root);
        assert_eq!(head.value, 1);

        // same as above
        let tail: Gc<'_, Node> =
            unsafe { Gc::from_custom_ptr(CustomPtr::from_raw(head.next_raw).unwrap()) };
        let tail_node = *cx.resolve(tail);
        assert_eq!(tail_node.value, 99);
        assert_eq!(tail_node.next_raw, 0);
    });
}

#[test]
fn drop_empty_pools() {
    let mut alloc = PoolAllocator4::new();
    alloc.mutate(|cx: AllocCtx<'_>| {
        let g1 = cx.try_alloc(1_u64).unwrap();
        let g2 = cx.try_alloc(2_u64).unwrap();
        let g3 = cx.try_alloc(3_u64).unwrap();
        assert_eq!(cx.live_slot_count(), 3);
        // each handle used exactly once
        unsafe {
            cx.free(g1);
            cx.free(g2);
            cx.free(g3);
        }
        assert_eq!(cx.live_slot_count(), 0);
    });
    for pool in &alloc.pools {
        assert!(pool.is_empty(), "pool {} not empty", pool.pool_id);
    }
}

#[test]
fn resolve_mut_mutates() {
    let mut alloc = PoolAllocator4::new();
    alloc.mutate(|cx: AllocCtx<'_>| {
        let gc = cx.try_alloc(0_u32).unwrap();
        // no other references to this slot
        unsafe { *cx.resolve_mut(gc) = 42 };
        assert_eq!(*cx.resolve(gc), 42);
    });
}

#[test]
fn multi_type_alloc() {
    let mut alloc = PoolAllocator4::new();
    alloc.mutate(|cx: AllocCtx<'_>| {
        let i = cx.try_alloc(i32::MIN).unwrap();
        let f = cx.try_alloc(1.5_f64).unwrap();
        let b = cx.try_alloc(true).unwrap();
        assert_eq!(*cx.resolve(i), i32::MIN);
        assert!((*cx.resolve(f) - 1.5_f64).abs() < f64::EPSILON);
        assert!(*cx.resolve(b));
    });
}

#[test]
fn option_customptr_niche() {
    assert_eq!(
        core::mem::size_of::<Option<CustomPtr>>(),
        core::mem::size_of::<CustomPtr>(),
    );
}

#[test]
fn alloc_after_deserialize() {
    // verify next_pool_id is correctly restored
    let (bytes, existing_ptr) = {
        let mut alloc = PoolAllocator4::new();
        let ptr = alloc.mutate(|cx: AllocCtx<'_>| cx.try_alloc(42_u32).unwrap().as_custom_ptr());
        (serialize(&alloc), ptr)
    };

    let mut alloc2 = deserialize(&bytes).unwrap();
    alloc2.mutate(|cx: AllocCtx<'_>| {
        // allocate a new value
        let new_gc = cx.try_alloc(99_u32).unwrap();
        let new_ptr = new_gc.as_custom_ptr();

        // pointers must not collide
        assert_ne!(
            new_ptr, existing_ptr,
            "new allocation collided with restored slot"
        );

        // existing_ptr came from a live allocation before serialization
        let old: &u32 = cx.resolve(unsafe { Gc::from_custom_ptr(existing_ptr) });
        assert_eq!(*old, 42);
        assert_eq!(*cx.resolve(new_gc), 99);
    });
}
