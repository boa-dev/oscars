use crate::collectors::mark_sweep::{MarkSweepGarbageCollector, TraceColor, trace::Trace};
use core::ops::{Deref, DerefMut};

// GC aware vector
//
// implements `Trace` so the gc can see its elements
//
// SAFETY:
//
// must be stored inside a `Gc<GcAllocVec<T>>` or similar traced container
// otherwise the memory may leak.
//
// EXAMPLE:
//
// ```ignore
// use oscars::{Gc, GcAllocVec, MarkSweepGarbageCollector};
//
// let collector = MarkSweepGarbageCollector::default();
// let vec = GcAllocVec::new(&collector);
// let gc_vec = Gc::new(vec, &collector);
//
// gc_vec.borrow_mut().push(42);
// collector.collect();
// assert_eq!(gc_vec.borrow()[0], 42);
// ```
#[derive(Debug)]
pub struct GcAllocVec<T> {
    inner: allocator_api2::vec::Vec<T, &'static MarkSweepGarbageCollector>,
}

impl<T> GcAllocVec<T> {
    #[inline]
    pub fn new_in(collector: &MarkSweepGarbageCollector) -> Self {
        Self {
            // SAFETY: GcAllocVec must be stored in a Gc<T> which ties its lifetime to the collector
            inner: allocator_api2::vec::Vec::new_in(unsafe {
                core::mem::transmute::<&MarkSweepGarbageCollector, &'static MarkSweepGarbageCollector>(
                    collector,
                )
            }),
        }
    }

    // creates a new empty `GcAllocVec` with capacity
    //
    // recommended to prevent wasted memory from reallocations
    #[inline]
    pub fn with_capacity(capacity: usize, collector: &MarkSweepGarbageCollector) -> Self {
        Self {
            inner: allocator_api2::vec::Vec::with_capacity_in(capacity, unsafe {
                core::mem::transmute::<&MarkSweepGarbageCollector, &'static MarkSweepGarbageCollector>(
                    collector,
                )
            }),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    // appends an element to the back
    //
    // repeated pushes that reallocate waste memory, use `with_capacity` when possible
    #[inline]
    pub fn push(&mut self, value: T) {
        self.inner.push(value);
    }

    // removes and returns the last element
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        self.inner.pop()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.inner.clear()
    }

    // returns a slice of the elements
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.inner
    }
}

impl<T> Deref for GcAllocVec<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for GcAllocVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: Trace> crate::collectors::mark_sweep::Finalize for GcAllocVec<T> {}

// SAFETY: GcAllocVec traces all its elements
unsafe impl<T: Trace> Trace for GcAllocVec<T> {
    unsafe fn trace(&self, color: TraceColor) {
        for element in self.inner.iter() {
            // SAFETY: called during mark phase only
            unsafe { element.trace(color) };
        }
    }

    fn run_finalizer(&self) {
        crate::collectors::mark_sweep::Finalize::finalize(self);
    }
}

// GC aware box
//
// implements `Trace` and must be stored inside a traced container
//
// EXAMPLE:
//
// ```ignore
// use oscars::{Gc, GcAllocBox, MarkSweepGarbageCollector};
//
// let collector = MarkSweepGarbageCollector::default();
// let boxed = GcAllocBox::new(42, &collector);
// let gc_box = Gc::new(boxed, &collector);
// ```
#[derive(Debug)]
pub struct GcAllocBox<T: ?Sized> {
    inner: allocator_api2::boxed::Box<T, &'static MarkSweepGarbageCollector>,
}

impl<T> GcAllocBox<T> {
    #[inline]
    pub fn new_in(value: T, collector: &MarkSweepGarbageCollector) -> Self {
        Self {
            inner: allocator_api2::boxed::Box::new_in(value, unsafe {
                core::mem::transmute::<&MarkSweepGarbageCollector, &'static MarkSweepGarbageCollector>(
                    collector,
                )
            }),
        }
    }
}

impl<T: ?Sized> Deref for GcAllocBox<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for GcAllocBox<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: Trace + ?Sized> crate::collectors::mark_sweep::Finalize for GcAllocBox<T> {}

// SAFETY: GcAllocBox traces its contents
unsafe impl<T: Trace + ?Sized> Trace for GcAllocBox<T> {
    unsafe fn trace(&self, color: TraceColor) {
        // SAFETY: called during mark phase only
        unsafe { (**self).trace(color) };
    }

    fn run_finalizer(&self) {
        crate::collectors::mark_sweep::Finalize::finalize(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::mark_sweep::{MarkSweepGarbageCollector, cell::GcRefCell, pointers::Gc};
    use rust_alloc::vec;

    #[test]
    fn gc_alloc_vec_basic() {
        let collector = &MarkSweepGarbageCollector::default();
        let vec = GcAllocVec::new_in(collector);
        let gc_vec = Gc::new_in(GcRefCell::new(vec), collector);

        gc_vec.borrow_mut().push(1u64);
        gc_vec.borrow_mut().push(2u64);
        gc_vec.borrow_mut().push(3u64);

        assert_eq!(gc_vec.borrow().len(), 3);
        assert_eq!(gc_vec.borrow()[0], 1);
        assert_eq!(gc_vec.borrow()[1], 2);
        assert_eq!(gc_vec.borrow()[2], 3);
    }

    #[test]
    fn gc_alloc_vec_survives_collection() {
        let collector = &mut MarkSweepGarbageCollector::default()
            .with_page_size(256)
            .with_heap_threshold(512);

        let vec = GcAllocVec::with_capacity(100, collector);
        let gc_vec = Gc::new_in(GcRefCell::new(vec), collector);

        for i in 0..100u64 {
            gc_vec.borrow_mut().push(i);
        }

        collector.collect();

        assert_eq!(gc_vec.borrow().len(), 100);
        assert_eq!(gc_vec.borrow()[50], 50);
    }

    #[test]
    fn gc_alloc_box_basic() {
        let collector = &MarkSweepGarbageCollector::default();
        let boxed = GcAllocBox::new_in(42u64, collector);
        let gc_box = Gc::new_in(GcRefCell::new(boxed), collector);

        assert_eq!(**gc_box.borrow(), 42);
    }

    #[test]
    fn gc_alloc_box_survives_collection() {
        let collector = &mut MarkSweepGarbageCollector::default();
        let data = vec![1, 2, 3, 4, 5];
        let boxed = GcAllocBox::new_in(data, collector);
        let gc_box = Gc::new_in(GcRefCell::new(boxed), collector);

        collector.collect();

        assert_eq!(gc_box.borrow().len(), 5);
        assert_eq!(gc_box.borrow()[2], 3);
    }

    #[test]
    fn gc_alloc_vec_with_gc_pointers() {
        let collector = &MarkSweepGarbageCollector::default();
        let vec = GcAllocVec::new_in(collector);
        let gc_vec = Gc::new_in(GcRefCell::new(vec), collector);

        let inner1 = Gc::new_in(GcRefCell::new(100u64), collector);
        let inner2 = Gc::new_in(GcRefCell::new(200u64), collector);

        gc_vec.borrow_mut().push(inner1.clone());
        gc_vec.borrow_mut().push(inner2.clone());

        drop(inner1);
        drop(inner2);

        collector.collect();

        // the gc pointers inside keep the values alive
        assert_eq!(gc_vec.borrow().len(), 2);
        assert_eq!(*gc_vec.borrow()[0].borrow(), 100);
        assert_eq!(*gc_vec.borrow()[1].borrow(), 200);
    }
}
