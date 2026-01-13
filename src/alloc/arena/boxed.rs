//! A `Box` type for the `ArenaAllocator`.

use core::marker::PhantomData;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

use crate::alloc::arena::ArenaPtr;
use crate::alloc::arena::finalize::Finalize;

pub struct Box<'arena, T: Finalize>(NonNull<T>, PhantomData<&'arena ()>);

impl<'arena, T: Finalize> Box<'arena, T> {
    pub fn from_arena_ptr(raw: ArenaPtr<'arena, T>) -> Self {
        Self(raw.to_non_null(), PhantomData)
    }

    pub fn into_raw(b: Self) -> *mut T {
        let mut b = mem::ManuallyDrop::new(b);
        &raw mut **b
    }
}

impl<'arena, T: Finalize> Finalize for Box<'arena, T> {
    fn finalize(&self) {
        unsafe { self.0.as_ref().finalize() };
    }
}

impl<'arena, T: Finalize> Drop for Box<'arena, T> {
    fn drop(&mut self) {
        // Run the finalizer on the fields of the box.
        Finalize::finalize(self);
        // TODO (nekevss): Can this cause a double free?
        //
        // MIRI appears to be alright with this general
        // construction, and we leak memory if we don't
        // drop any heap allocated memory that is part
        // of T.
        //
        // SAFETY: We own the underlying data, so it is
        // valid to drop anything connected to T
        unsafe {
            core::ptr::drop_in_place(self.0.as_mut());
        }
    }
}

impl<'arena, T: Finalize> DerefMut for Box<'arena, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Box<T> is valid for the life of the Arena.
        unsafe { self.0.as_mut() }
    }
}

impl<'arena, T: Finalize> Deref for Box<'arena, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}
