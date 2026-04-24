
//! Compile-fail: `Root<'id, T>` from one `with_gc` context cannot be used
//! inside a different `with_gc` context.
//!
//! `with_gc` has a `for<'id>` bound, so every call produces a fresh, unnamed
//! `'id` lifetime.  `Root<'id1, T>` and `MutationContext<'id2, '_>` carry
//! distinct, non-unifiable `'id` variables, so the borrow checker rejects
//! `root.get(cx)` when `root` and `cx` come from different contexts.

use oscars::collectors::mark_sweep_branded::with_gc;

fn main() {
    with_gc(|ctx1| {
        with_gc(|ctx2| {
            // root carries 'id of ctx1
            let root = ctx1.mutate(|cx| cx.root(cx.alloc(123i32)));

            ctx2.mutate(|cx| {
                // ERROR: `'id` of `root` (ctx1) != `'id` of `cx` (ctx2)
                let _gc = root.get(cx);
            });
        });
    });
}
