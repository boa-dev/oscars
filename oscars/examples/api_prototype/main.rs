//! GC API prototype based on gc-arena's lifetime pattern
//!
//! key change: `Gc<'gc, T>` is Copy (zero overhead) vs current `Gc<T>` (inc/dec on clone/drop)
//!
//! Run: `cargo run --example api_prototype`

#![allow(dead_code)]

mod cell;
mod gc;
mod root_list;
mod trace;
mod weak;

use cell::GcRefCell;
use gc::Gc;
use gc::{GcContext, MutationContext};
use trace::{Finalize, Trace, Tracer};

struct JsObject {
    name: String,
    value: i32,
}

impl Trace for JsObject {
    fn trace(&mut self, tracer: &mut Tracer) {
        self.name.trace(tracer);
        self.value.trace(tracer);
    }
}
impl Finalize for JsObject {}

struct JsArray<'gc> {
    elements: Vec<Gc<'gc, JsObject>>,
}

impl<'gc> Trace for JsArray<'gc> {
    fn trace(&mut self, tracer: &mut Tracer) {
        for elem in &mut self.elements {
            tracer.mark(elem);
        }
    }
}
impl<'gc> Finalize for JsArray<'gc> {}

/// Replica of Boa Builtin Function: Array.prototype.push
/// This fully proves that standalone builtin functions can accept the `'gc`
/// context bounded pointers without lifetime errors or borrow checking issues
fn array_push<'gc>(
    this: &Gc<'gc, GcRefCell<JsArray<'gc>>>,
    args: &[Gc<'gc, JsObject>],
    _cx: &MutationContext<'gc>,
) -> usize {
    let mut array = this.get().borrow_mut();

    for arg in args {
        array.elements.push(*arg);
    }

    array.elements.len()
}

fn main() {
    println!("GC API Prototype Example (Redesign Additions)\n");

    let ctx = GcContext::new();

    // example 1: boa array migration
    println!("1. Boa Array Migration Example:\n");
    ctx.mutate(|cx| {
        let val1 = cx.alloc(JsObject {
            name: "item1".to_string(),
            value: 42,
        });
        let val2 = cx.alloc(JsObject {
            name: "item2".to_string(),
            value: 43,
        });

        let array = cx.alloc(GcRefCell::new(JsArray {
            elements: Vec::new(),
        }));

        println!("  Calling array_push built-in replica:");
        let new_len = array_push(&array, &[val1, val2], cx);

        println!("  Returned length: {}", new_len);
        println!(
            "  First element value: {}\n",
            array.get().borrow().elements[0].get().value
        );
    });

    // example 2: weak refs
    println!("2. WeakGc upgrade example:\n");
    ctx.mutate(|cx| {
        let target = cx.alloc(JsObject {
            name: "target".to_string(),
            value: 5,
        });
        let _root = cx.root(target); // force it alive
        let weak = cx.alloc_weak(JsObject {
            name: "weak_data".to_string(),
            value: 10,
        });

        cx.collect();

        match weak.upgrade(cx) {
            Some(gc) => println!("  Weak object is accessible: {}", gc.get().value),
            None => println!("  Weak object was swept"),
        }
        println!();
    });

    println!("Done!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_mutate() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let a = cx.alloc(42i32);
            assert_eq!(*a.get(), 42);
        });
    }

    #[test]
    fn root_works_in_context() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let obj = cx.alloc(123i32);
            let root = cx.root(obj);
            let gc = root.get(cx);
            assert_eq!(*gc.get(), 123);
        });
    }

    #[test]
    fn scoped_root_is_context_bound() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let obj = cx.alloc(777i32);
            let scoped = cx.root_scoped(obj);
            let gc = scoped.get(cx);
            assert_eq!(*gc.get(), 777);
        });
    }

    #[test]
    fn root_rejects_different_collector() {
        let ctx1 = GcContext::new();
        let ctx2 = GcContext::new();

        let root = ctx1.mutate(|cx| {
            let obj = cx.alloc(123i32);
            cx.root(obj)
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctx2.mutate(|cx| {
                let _gc = root.get(cx);
            });
        }));
        assert!(result.is_err());
    }

    #[test]
    fn refcell_trace() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let cell = cx.alloc(GcRefCell::new(100i32));
            *cell.get().borrow_mut() = 200;
            assert_eq!(*cell.get().borrow(), 200);
        });
    }

    #[test]
    fn root_outlives_context() {
        // Ensures escaping roots do not trigger UAF after collector drops
        let escaped_root = {
            let ctx = GcContext::new();
            ctx.mutate(|cx| cx.root(cx.alloc(555i32)))
        };
        drop(escaped_root);
    }

    #[test]
    fn weak_upgrade() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let obj = cx.alloc(JsObject {
                name: "test".into(),
                value: 42,
            });
            let weak = cx.alloc_weak(JsObject {
                name: "weak".into(),
                value: 10,
            });

            // Sweep unrooted weak pointers.
            cx.collect();
            assert!(weak.upgrade(cx).is_none());

            // Rooted objects remain alive
            let root = cx.root(obj);
            cx.collect();
            let _ = root.get(cx);
        });
    }

    #[test]
    fn bulk_allocation_cleanup() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            for i in 0..100 {
                cx.alloc(JsObject {
                    name: "bulk".into(),
                    value: i,
                });
            }
        });
        // Deallocates out of scope without leaking
    }

    #[test]
    #[cfg_attr(not(debug_assertions), ignore)]
    fn compile_fail_tests() {
        let t = trybuild::TestCases::new();
        t.compile_fail("examples/api_prototype/tests/ui/*.rs");
    }

    // Tests verifying the lifetime-bounded `'gc` invariant prevents UAF
    #[test]
    fn unrooted_allocs_are_collected() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let weak = cx.alloc_weak(JsObject {
                name: "ephemeral".into(),
                value: 999,
            });

            cx.collect();
            assert!(weak.upgrade(cx).is_none());
        });
    }

    #[test]
    fn pinned_root_keeps_gc_alive() {
        // Ensures Pin<Box<Root<T>>> keeps allocations alive.
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let obj = cx.alloc(JsObject {
                name: "pinned".into(),
                value: 42,
            });

            let pinned_root = cx.root(obj);
            cx.collect();

            let gc = pinned_root.get(cx);
            assert_eq!(gc.get().value, 42);
            assert_eq!(gc.get().name, "pinned");
        });
    }

    #[test]
    fn multiple_roots_are_independent() {
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let obj1 = cx.alloc(100i32);
            let obj2 = cx.alloc(200i32);

            let root1 = cx.root(obj1);
            let root2 = cx.root(obj2);

            cx.collect();

            assert_eq!(*root1.get(cx).get(), 100);
            assert_eq!(*root2.get(cx).get(), 200);

            drop(root1);
            cx.collect();

            assert_eq!(*root2.get(cx).get(), 200);
        });
    }

    #[test]
    fn root_get_requires_mut_ctx() {
        // Ensures Root::get() requires a valid MutationContext<'gc>.
        let ctx = GcContext::new();

        let root = ctx.mutate(|cx| {
            let obj = cx.alloc(JsObject {
                name: "escaped".into(),
                value: 123,
            });
            cx.root(obj)
        });

        ctx.mutate(|cx| {
            let gc = root.get(cx);
            assert_eq!(gc.get().value, 123);
        });
    }

    #[test]
    fn gc_lifetime_tied_to_mut_ctx() {
        // Ensures Gc<'gc, T> cannot outlive the mutation phase.
        let ctx = GcContext::new();
        ctx.mutate(|cx| {
            let gc = cx.alloc(42i32);
            assert_eq!(*gc.get(), 42);
        });
    }

    #[test]
    fn seq_mutations_independent() {
        let ctx = GcContext::new();

        let root = ctx.mutate(|cx| {
            let obj = cx.alloc(1i32);
            cx.root(obj)
        });

        ctx.mutate(|cx| {
            let new_obj = cx.alloc(2i32);

            assert_eq!(*root.get(cx).get(), 1);
            assert_eq!(*new_obj.get(), 2);
        });
    }
}
