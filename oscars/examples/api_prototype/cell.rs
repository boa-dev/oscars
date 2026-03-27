use crate::trace::{Finalize, Trace, Tracer};
use core::cell::{Ref, RefCell, RefMut};

pub struct GcRefCell<T: Trace> {
    inner: RefCell<T>,
}

impl<T: Trace> GcRefCell<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
    }
    pub fn borrow(&self) -> GcRef<'_, T> {
        GcRef(self.inner.borrow())
    }
    pub fn borrow_mut(&self) -> GcRefMut<'_, T> {
        GcRefMut(self.inner.borrow_mut())
    }
}

pub struct GcRef<'a, T: Trace>(Ref<'a, T>);
impl<'a, T: Trace> core::ops::Deref for GcRef<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct GcRefMut<'a, T: Trace>(RefMut<'a, T>);
impl<'a, T: Trace> core::ops::Deref for GcRefMut<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<'a, T: Trace> core::ops::DerefMut for GcRefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Trace> Trace for GcRefCell<T> {
    fn trace(&mut self, tracer: &mut Tracer) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.trace(tracer);
        }
    }
}
impl<T: Trace> Finalize for GcRefCell<T> {}
