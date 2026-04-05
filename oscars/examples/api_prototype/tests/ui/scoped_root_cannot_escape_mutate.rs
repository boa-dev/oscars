//! Compile-fail test: ScopedRoot<'gc, T> cannot escape mutate()

struct GcContext;
struct MutationContext<'gc>(&'gc ());
struct Gc<'gc, T>(&'gc T);
struct ScopedRoot<'gc, T>(Gc<'gc, T>);

impl GcContext {
    fn new() -> Self {
        GcContext
    }
    fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'gc>) -> R) -> R {
        f(&MutationContext(&()))
    }
}

impl<'gc> MutationContext<'gc> {
    fn alloc<T>(&self, _v: T) -> Gc<'gc, T> {
        todo!()
    }
    fn root_scoped<T>(&self, gc: Gc<'gc, T>) -> ScopedRoot<'gc, T> {
        ScopedRoot(gc)
    }
}

fn main() {
    let ctx = GcContext::new();

    let escaped = ctx.mutate(|cx| {
        let gc = cx.alloc(42i32);
        cx.root_scoped(gc)
    });

    let _ = escaped;
}
