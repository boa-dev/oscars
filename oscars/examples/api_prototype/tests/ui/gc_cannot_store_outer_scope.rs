//! Compile-fail test: Gc reference cannot be stored beyond 'gc lifetime
//!
//! This demonstrates that the 'gc lifetime prevents storing Gc pointers
//! in locations that outlive the mutation context.

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

struct Holder<'a> {
    gc: Gc<'a, i32>,
}

fn main() {
    let ctx = GcContext::new();
    let mut holder: Option<Holder<'_>> = None;
    
    // This MUST fail: can't store Gc in outer scope
    ctx.mutate(|cx| {
        let gc = cx.alloc(42);
        holder = Some(Holder { gc });  // ERROR: 'gc does not live long enough
    });
}
