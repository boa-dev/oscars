use super::*;

#[test]
fn ephemeron_value_survives_when_key_is_rooted() {
    with_gc(|ctx| {
        let (root_key, eph) = ctx.mutate(|cx| {
            let key = cx.alloc(1u32);
            let value = cx.alloc(42u32);
            let root_key = cx.root(key);
            let eph = cx.alloc_ephemeron(key, value);
            (root_key, eph)
        });

        ctx.collect();

        ctx.mutate(|cx| {
            let val = eph
                .get_value(cx)
                .expect("value must be alive while key is rooted");
            assert_eq!(*val.get(), 42);
            drop(root_key);
        });
    });
}

#[test]
fn ephemeron_value_collected_when_key_unrooted() {
    with_gc(|ctx| {
        let eph = ctx.mutate(|cx| {
            let key = cx.alloc(1u32);
            let value = cx.alloc(99u32);
            cx.alloc_ephemeron(key, value)
        });

        ctx.collect();

        ctx.mutate(|cx| {
            assert!(
                eph.get_value(cx).is_none(),
                "value must be gone after key is swept"
            );
        });
    });
}

#[test]
fn ephemeron_chain_fixpoint() {
    // Ephemeron(root_a -> b) and Ephemeron(b -> c):
    // root_a alive -> b survives via first ephemeron.
    // b then alive -> c survives via second ephemeron
    // This requires the collector to run multiple fixpoint passes.
    with_gc(|ctx| {
        let (root_a, eph_ab, eph_bc) = ctx.mutate(|cx| {
            let a = cx.alloc(1u32);
            let b = cx.alloc(2u32);
            let c = cx.alloc(3u32);
            let root_a = cx.root(a);
            let eph_ab = cx.alloc_ephemeron(a, b);
            let eph_bc = cx.alloc_ephemeron(b, c);
            (root_a, eph_ab, eph_bc)
        });

        ctx.collect();

        ctx.mutate(|cx| {
            let b_val = eph_ab.get_value(cx).expect("b must survive: a is rooted");
            assert_eq!(*b_val.get(), 2);
            let c_val = eph_bc
                .get_value(cx)
                .expect("c must survive: b is kept alive by ephemeron");
            assert_eq!(*c_val.get(), 3);
        });

        drop(root_a);
        ctx.collect();

        ctx.mutate(|cx| {
            assert!(
                eph_ab.get_value(cx).is_none(),
                "b must be gone after a is dropped"
            );
            assert!(
                eph_bc.get_value(cx).is_none(),
                "c must be gone after b is gone"
            );
        });
    });
}

#[test]
fn ephemeron_entry_cleaned_up_after_sweep() {
    // Verify the collector's internal ephemeron list shrinks after dead entries are swept.
    with_gc(|ctx| {
        ctx.mutate(|cx| {
            let key = cx.alloc(0u32);
            let value = cx.alloc(0u32);
            cx.alloc_ephemeron(key, value);
        });
        assert_eq!(ctx.ephemeron_count(), 1);

        ctx.collect();

        assert_eq!(
            ctx.ephemeron_count(),
            0,
            "dead ephemeron entry must be removed after sweep"
        );
    });
}
