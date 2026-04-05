//! Compile-fail test: `Root<'id, T>` cannot be used across `with_gc` contexts.
//!
//! Each `with_gc` call creates a unique, unnamed `'id` lifetime. The borrow checker 
//! cannot unify two distinct `'id` variables, so passing a `Root<'id1, T>` to a 
//!`MutationContext<'id2, '_>` is a type error.

use core::marker::PhantomData;

struct Root<'id, T> {
    _marker: PhantomData<(*mut &'id (), T)>,
}

impl<'id, T> Root<'id, T> {
    fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> &'gc T {
        todo!()
    }
}

struct MutationContext<'id, 'gc> {
    _marker: PhantomData<(*mut &'id (), &'gc ())>,
}

impl<'id, 'gc> MutationContext<'id, 'gc> {
    fn alloc<T>(&self, _v: T) -> T { todo!() }
    fn root<T>(&self, _v: T) -> Root<'id, T> { todo!() }
}

struct GcContext<'id> {
    _marker: PhantomData<*mut &'id ()>,
}

impl<'id> GcContext<'id> {
    fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'id, 'gc>) -> R) -> R {
        f(&MutationContext { _marker: PhantomData })
    }
}

fn with_gc<R, F: for<'id> FnOnce(GcContext<'id>) -> R>(f: F) -> R {
    f(GcContext { _marker: PhantomData })
}

fn main() {
    // Each with_gc produces a fresh 'id; the root from ctx1 cannot be used
    // with the MutationContext from ctx2, this must fail to compile.
    with_gc(|ctx1| {
        with_gc(|ctx2| {
            let root = ctx1.mutate(|cx| cx.root(cx.alloc(123i32)));
            ctx2.mutate(|cx| {
                let _gc = root.get(cx); // ERROR: 'id mismatch
            });
        });
    });
}
