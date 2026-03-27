use crate::trace::{Finalize, Trace};
use crate::weak::WeakGc;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COLLECTOR_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct GcBox<T: ?Sized> {
    pub(crate) marked: Cell<bool>,
    pub(crate) value: T,
}

#[derive(Clone)]
pub(crate) struct RootEntry {
    pub(crate) ptr: *mut u8,
}

#[derive(Debug)]
pub struct Gc<'gc, T: Trace + ?Sized + 'gc> {
    pub(crate) ptr: NonNull<GcBox<T>>,
    pub(crate) _marker: PhantomData<(&'gc T, *const ())>,
}

impl<'gc, T: Trace + ?Sized + 'gc> Copy for Gc<'gc, T> {}
impl<'gc, T: Trace + ?Sized + 'gc> Clone for Gc<'gc, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'gc, T: Trace + 'gc> Gc<'gc, T> {
    pub fn get(&self) -> &T {
        unsafe { &(*self.ptr.as_ptr()).value }
    }
}

pub struct Root<T: Trace + ?Sized> {
    pub(crate) ptr: NonNull<GcBox<T>>,
    pub(crate) collector_id: u64,
    pub(crate) collector_roots: Rc<RefCell<Vec<RootEntry>>>,
    pub(crate) _marker: PhantomData<*const ()>,
}

impl<T: Trace> Root<T> {
    pub fn get<'gc>(&self, cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        assert_eq!(
            self.collector_id, cx.collector.id,
            "root from different collector"
        );
        Gc {
            ptr: self.ptr,
            _marker: PhantomData,
        }
    }
    pub fn belongs_to(&self, cx: &MutationContext<'_>) -> bool {
        self.collector_id == cx.collector.id
    }
}

impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        self.collector_roots
            .borrow_mut()
            .retain(|entry| entry.ptr != self.ptr.as_ptr() as *mut u8);
    }
}

struct Allocation {
    ptr: *mut u8,
    drop_fn: unsafe fn(*mut u8),
}

pub struct Collector {
    pub(crate) id: u64,
    allocations: RefCell<Vec<Allocation>>,
    pub(crate) roots: Rc<RefCell<Vec<RootEntry>>>,
    allocation_count: Cell<usize>,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            id: NEXT_COLLECTOR_ID.fetch_add(1, Ordering::Relaxed),
            allocations: RefCell::new(Vec::new()),
            roots: Rc::new(RefCell::new(Vec::new())),
            allocation_count: Cell::new(0),
        }
    }

    pub(crate) fn alloc<'gc, T: Trace + Finalize + 'gc>(&'gc self, value: T) -> Gc<'gc, T> {
        let boxed = Box::new(GcBox {
            marked: Cell::new(false),
            value,
        });
        let ptr = NonNull::new(Box::into_raw(boxed)).unwrap();

        unsafe fn drop_alloc<T>(ptr: *mut u8) {
            unsafe {
                drop(Box::from_raw(ptr as *mut GcBox<T>));
            }
        }

        self.allocations.borrow_mut().push(Allocation {
            ptr: ptr.as_ptr() as *mut u8,
            drop_fn: drop_alloc::<T>,
        });

        self.allocation_count.set(self.allocation_count.get() + 1);
        Gc {
            ptr,
            _marker: PhantomData,
        }
    }

    pub(crate) fn add_root<T: Trace + Finalize + ?Sized>(&self, ptr: NonNull<GcBox<T>>) {
        self.roots.borrow_mut().push(RootEntry {
            ptr: ptr.as_ptr() as *mut u8,
        });
    }

    pub(crate) fn collect(&self) {
        let root_count = self.roots.borrow().len();
        println!(
            "Collecting garbage: {} objects, {} roots",
            self.allocation_count.get(),
            root_count
        );
        for entry in self.roots.borrow().iter() {
            unsafe {
                let gcbox = entry.ptr as *mut GcBox<()>;
                (*gcbox).marked.set(true);
            }
        }
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        for alloc in self.allocations.borrow().iter() {
            unsafe {
                (alloc.drop_fn)(alloc.ptr);
            }
        }
    }
}

pub struct GcContext {
    collector: Collector,
}

impl GcContext {
    pub fn new() -> Self {
        Self {
            collector: Collector::new(),
        }
    }
    pub fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'gc>) -> R) -> R {
        let cx = MutationContext {
            collector: &self.collector,
            _marker: PhantomData,
        };
        f(&cx)
    }
}

pub struct MutationContext<'gc> {
    pub(crate) collector: &'gc Collector,
    pub(crate) _marker: PhantomData<*const ()>,
}

impl<'gc> MutationContext<'gc> {
    pub fn alloc<T: Trace + Finalize + 'gc>(&self, value: T) -> Gc<'gc, T> {
        self.collector.alloc(value)
    }

    pub fn alloc_weak<T: Trace + Finalize + 'gc>(&self, value: T) -> WeakGc<T> {
        let gc = self.alloc(value);
        WeakGc { ptr: gc.ptr }
    }

    pub fn root<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> Root<T> {
        self.collector.add_root(gc.ptr);
        Root {
            ptr: gc.ptr,
            collector_id: self.collector.id,
            collector_roots: Rc::clone(&self.collector.roots),
            _marker: PhantomData,
        }
    }

    pub fn collector_id(&self) -> u64 {
        self.collector.id
    }
    pub fn collect(&self) {
        self.collector.collect();
    }
}
