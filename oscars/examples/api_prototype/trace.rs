use crate::gc::Gc;
use core::marker::PhantomData;

pub trait Finalize {
    fn finalize(&self) {}
}

pub trait Trace {
    fn trace(&mut self, tracer: &mut Tracer);
}

pub struct Tracer<'a> {
    pub(crate) _marker: PhantomData<&'a ()>,
}

impl Tracer<'_> {
    pub fn mark<T: Trace + ?Sized>(&mut self, _gc: &mut Gc<'_, T>) {}
}

impl Trace for i32 {
    fn trace(&mut self, _: &mut Tracer) {}
}
impl Finalize for i32 {}

impl Trace for String {
    fn trace(&mut self, _: &mut Tracer) {}
}
impl Finalize for String {}
