//! Compile fail test: Gc<'gc, T> cannot escape the mutate() closure
//!
//! The safety is enforced at compile time via the 'gc lifetime.

struct GcContext;
struct MutationContext<'gc>(&'gc ());
struct Gc<'gc, T>(&'gc T);

impl GcContext {
    fn new() -> Self { GcContext }
    fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'gc>) -> R) -> R {
        f(&MutationContext(&()))
    }
}

impl<'gc> MutationContext<'gc> {
    fn alloc<T>(&self, _v: T) -> Gc<'gc, T> { todo!() }
}

fn main() {
    let ctx = GcContext::new();
    
    // This MUST fail to compile: Gc cannot escape mutate()
    let escaped = ctx.mutate(|cx| {
        cx.alloc(42i32)  // Gc<'gc, i32> tied to closure lifetime
    });
    
    // If this compiled, we could access dead memory
    let _ = escaped;
}
