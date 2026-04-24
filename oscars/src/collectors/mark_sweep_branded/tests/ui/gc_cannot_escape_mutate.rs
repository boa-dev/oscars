//! Compile-fail: `Gc<'gc, T>` cannot escape the `mutate()` closure.
//!
//! `for<'gc>` in `GcContext::mutate` makes `'gc` universally quantified inside
//! the closure. The return type `R` cannot mention `'gc`, so the borrow checker
//! rejects any attempt to return a `Gc<'gc, T>` from `mutate`.

use oscars::collectors::mark_sweep_branded::with_gc;

fn main() {
    with_gc(|ctx| {
        // The closure must return R for some R that does not mention 'gc.
        // Returning Gc<'gc, i32> directly attempts to leak a shorter lifetime
        // into the outer scope. The compiler must reject this.
        let _escaped = ctx.mutate(|cx| {
            cx.alloc(42i32) // ERROR: Gc<'gc, i32> cannot escape mutate()
        });
    });
}
