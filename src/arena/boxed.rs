//! A `Box` type for the `ArenaAllocator`.

use core::mem;
use core::ops::{DerefMut, Deref};
use core::ptr::NonNull;
use std::ptr;

use crate::arena::finalize::Finalize;


pub struct Box<T: Finalize>(NonNull<T>);

impl<T: Finalize> Box<T> {
    pub unsafe fn from_raw(raw: *mut T) -> Self {
        // Safety: Caller must guarantee that *mut T is a valid pointer.
        Self(unsafe {NonNull::new_unchecked(raw) })
    }

    pub fn into_raw(b: Self) -> *mut T {
        let mut b = mem::ManuallyDrop::new(b);
        &raw mut **b
    }
}

impl<T: Finalize> Finalize for Box<T> {
    fn finalize(&self) {
        unsafe { self.0.as_ref().finalize() };
    }
}

impl<T: Finalize> Drop for Box<T> {
    fn drop(&mut self) {
        // Run the finalizer on the fields of the box. 
        Finalize::finalize(self);
        // SAFETY: TODO - is this a double free?
        unsafe {
            core::ptr::drop_in_place(self.0.as_mut());
        }
    }
}

impl<T: Finalize> DerefMut for Box<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.as_mut() }
    }
}

impl<T: Finalize> Deref for Box<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { self.0.as_ref() }
    }
}


