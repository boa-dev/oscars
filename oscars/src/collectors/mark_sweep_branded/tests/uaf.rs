use crate::collectors::mark_sweep::Finalize;
use crate::collectors::mark_sweep_branded::Trace;
use crate::collectors::mark_sweep_branded::with_gc;
use core::cell::Cell;

struct DetectDrop<'a>(&'a Cell<bool>);

impl<'a> Trace for DetectDrop<'a> {
    fn trace(&self, _color: &crate::collectors::mark_sweep_branded::trace::TraceColor) {}
}

impl Finalize for DetectDrop<'_> {}

impl Drop for DetectDrop<'_> {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[test]
fn test_uaf() {
    with_gc(|cx| {
        let dropped = Cell::new(false);
        cx.mutate(|mcx| {
            let _gc = mcx.alloc(DetectDrop(&dropped));
        });
        cx.collect(); // Garbage collects 'gc' because it isn't rooted!
        assert!(dropped.get(), "It wasn't collected!");
        // The pointer _gc is safely out of scope here! Compiler prevents accessing it.
    });
}
