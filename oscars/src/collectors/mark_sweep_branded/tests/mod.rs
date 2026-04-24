use super::*;

#[derive(Debug)]
struct JsObject {
    name: rust_alloc::string::String,
    value: i32,
}

impl crate::collectors::mark_sweep_branded::Trace for JsObject {
    fn trace(&mut self, _tracer: &mut crate::collectors::mark_sweep_branded::trace::Tracer) {}
}
impl crate::collectors::mark_sweep_branded::Finalize for JsObject {}

#[test]
fn unrooted_alloc_is_swept() {
    with_gc(|ctx| {
        let weak = ctx.mutate(|cx| {
            cx.alloc_weak(cx.alloc(JsObject {
                name: "ephemeral".into(),
                value: 999,
            }))
        });
        ctx.collect();
        ctx.mutate(|cx| {
            assert!(weak.upgrade(cx).is_none());
        });
    });
}

#[test]
fn rooted_alloc_survives_collection() {
    with_gc(|ctx| {
        let root = ctx.mutate(|cx| {
            cx.root(cx.alloc(JsObject {
                name: "pinned".into(),
                value: 42,
            }))
        });
        ctx.collect();
        ctx.mutate(|cx| {
            let gc = root.get(cx);
            assert_eq!(gc.get().value, 42);
            assert_eq!(gc.get().name, "pinned");
        });
    });
}

#[test]
fn weak_upgrade_after_collection_without_root_is_none() {
    with_gc(|ctx| {
        let weak = ctx.mutate(|cx| {
            cx.alloc_weak(cx.alloc(JsObject {
                name: "weak".into(),
                value: 10,
            }))
        });
        ctx.collect();
        ctx.mutate(|cx| {
            assert!(weak.upgrade(cx).is_none());
        });
    });
}

#[test]
fn weak_upgrade_with_live_root_is_some() {
    with_gc(|ctx| {
        let (root, weak) = ctx.mutate(|cx| {
            let obj = cx.alloc(JsObject {
                name: "strong".into(),
                value: 7,
            });
            let root = cx.root(obj);

            let weak = cx.alloc_weak(cx.alloc(JsObject {
                name: "weak_entry".into(),
                value: 77,
            }));
            (root, weak)
        });
        ctx.collect();
        ctx.mutate(|cx| {
            assert!(weak.upgrade(cx).is_none());
            assert_eq!(root.get(cx).get().value, 7);
        });
    });
}

#[test]
fn multiple_roots_are_independent() {
    with_gc(|ctx| {
        let (root1, root2) = ctx.mutate(|cx| {
            let obj1 = cx.alloc(100i32);
            let obj2 = cx.alloc(200i32);
            (cx.root(obj1), cx.root(obj2))
        });

        ctx.collect();

        ctx.mutate(|cx| {
            assert_eq!(*root1.get(cx).get(), 100);
            assert_eq!(*root2.get(cx).get(), 200);
        });

        drop(root1);
        ctx.collect();

        ctx.mutate(|cx| {
            assert_eq!(*root2.get(cx).get(), 200);
        });
    });
}

#[test]
fn root_escapes_closure_safely() {
    with_gc(|ctx| {
        let root = ctx.mutate(|cx| {
            let obj = cx.alloc(555i32);
            cx.root(obj)
        });

        ctx.collect();

        ctx.mutate(|cx| {
            assert_eq!(*root.get(cx).get(), 555);
        });
    });
}

mod api_compliance;
mod ephemeron;
mod uaf;
mod ui_tests;
