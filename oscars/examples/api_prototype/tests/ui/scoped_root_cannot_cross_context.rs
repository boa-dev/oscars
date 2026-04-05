//! Compile-fail test: with context branding, a root from brand A cannot be used
//! with a mutation context from brand B.

use core::marker::PhantomData;

struct BrandA;
struct BrandB;

struct GcContext<B>(PhantomData<B>);
struct MutationContext<'gc, B>(&'gc (), PhantomData<B>);
struct Gc<'gc, T>(&'gc T);
struct ScopedRoot<'gc, B, T>(Gc<'gc, T>, PhantomData<B>);

impl<B> GcContext<B> {
    fn new() -> Self {
        GcContext(PhantomData)
    }
    fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'gc, B>) -> R) -> R {
        f(&MutationContext(&(), PhantomData))
    }
}

impl<'gc, B, T> ScopedRoot<'gc, B, T> {
    fn get(&self, _cx: &MutationContext<'gc, B>) -> Gc<'gc, T> {
        todo!()
    }
}

impl<'gc, B> MutationContext<'gc, B> {
    fn alloc<T>(&self, _v: T) -> Gc<'gc, T> {
        todo!()
    }
    fn root_scoped<T>(&self, gc: Gc<'gc, T>) -> ScopedRoot<'gc, B, T> {
        ScopedRoot(gc, PhantomData)
    }
}

fn main() {
    let ctx1 = GcContext::<BrandA>::new();
    let ctx2 = GcContext::<BrandB>::new();

    ctx1.mutate(|cx1| {
        let gc = cx1.alloc(42i32);
        let root = cx1.root_scoped(gc);

        ctx2.mutate(|cx2| {
            let _ = root.get(cx2);
        });
    });
}
