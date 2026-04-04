use crate::gc::{Gc, GcBox, MutationContext};
use crate::trace::{Finalize, Trace, Tracer};
use core::marker::PhantomData;
use core::ptr::NonNull;

pub struct WeakGc<T: Trace + ?Sized> {
    pub(crate) ptr: NonNull<GcBox<T>>,
}

impl<T: Trace + ?Sized> WeakGc<T> {
    pub fn upgrade<'gc>(&self, _cx: &MutationContext<'gc>) -> Option<Gc<'gc, T>> {
        unsafe {
            let marked = (*self.ptr.as_ptr()).marked.get();
            if marked {
                Some(Gc {
                    ptr: self.ptr,
                    _marker: PhantomData,
                })
            } else {
                None
            }
        }
    }
}

impl<T: Trace + ?Sized> Clone for WeakGc<T> {
    fn clone(&self) -> Self {
        Self { ptr: self.ptr }
    }
}

pub struct WeakMap<K: Trace, V: Trace> {
    _marker: PhantomData<(K, V)>,
}

impl<K: Trace, V: Trace> WeakMap<K, V> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<K: Trace, V: Trace> Trace for WeakMap<K, V> {
    fn trace(&mut self, _tracer: &mut Tracer) {}
}
impl<K: Trace, V: Trace> Finalize for WeakMap<K, V> {}
