
//! Compile-fail: `Gc<'gc, T>` cannot be stored in a location that outlives
//! the `mutate()` closure.
//!
//! Even if the caller does not return the `Gc` directly, storing it in an
//! outer `let` binding that outlives the closure is equally rejected: the
//! `'gc` lifetime of the `Gc` is shorter than the outer binding's scope.

use oscars::collectors::mark_sweep_branded::{Gc, with_gc};

struct Holder<'a> {
    gc: Gc<'a, i32>,
}

fn main() {
    with_gc(|ctx| {
        let mut holder: Option<Holder<'_>> = None;

        ctx.mutate(|cx| {
            let gc = cx.alloc(42i32);
            // ERROR: `cx` (and therefore `gc`'s `'gc` lifetime) does not live
            // long enough to be stored in `holder`.
            holder = Some(Holder { gc });
        });

        let _ = holder;
    });
}
