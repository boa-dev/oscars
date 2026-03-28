use crate::root_list::{RootList, RootListNode};
use crate::trace::{Finalize, Trace};
use crate::weak::WeakGc;
use core::alloc::Layout;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::pin::Pin;
use core::ptr::NonNull;
use oscars::alloc::mempool3::PoolAllocator;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COLLECTOR_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct GcBox<T: ?Sized> {
    pub(crate) marked: Cell<bool>,
    pub(crate) value: T,
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

/// Pinned root handle for GC allocations that outlive `'gc`
///
/// Uses intrusive linked list for O(1) drop. Requires Pin for stable addresses
/// Safety:
// `'gc` lifetime ensures Gc pointers don't outlive collection
#[must_use = "roots must be kept alive to prevent collection"]
pub struct Root<T: Trace + ?Sized> {
    pub(crate) gc_ptr: NonNull<GcBox<T>>,
    pub(crate) collector_id: u64,
    pub(crate) collector_roots: Rc<RefCell<RootList>>,
    pub(crate) node: RootListNode,
    pub(crate) _marker: PhantomData<*const ()>,
}

impl<T: Trace> Root<T> {
    pub fn get<'gc>(&self, cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        assert_eq!(
            self.collector_id, cx.collector.id,
            "root from different collector"
        );
        Gc {
            ptr: self.gc_ptr,
            _marker: PhantomData,
        }
    }

    pub fn belongs_to(&self, cx: &MutationContext<'_>) -> bool {
        self.collector_id == cx.collector.id
    }
}

impl<T: Trace + ?Sized> Drop for Root<T> {
    fn drop(&mut self) {
        // SAFETY:
        // node_ptr is valid because self is being dropped (still alive).
        // Using borrow() is correct: RootList uses interior mutability via Cell
        unsafe {
            let node_ptr = NonNull::new_unchecked(&self.node as *const _ as *mut RootListNode);
            self.collector_roots.borrow().remove(node_ptr);
        }
    }
}

struct PoolEntry {
    ptr: NonNull<u8>,
    drop_fn: unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>),
}

/// GC collector using mempool3 for size-class pooling
pub struct Collector {
    pub(crate) id: u64,
    pool: RefCell<PoolAllocator<'static>>,
    pool_entries: RefCell<Vec<PoolEntry>>,
    pub(crate) roots: Rc<RefCell<RootList>>,
    allocation_count: Cell<usize>,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            id: NEXT_COLLECTOR_ID.fetch_add(1, Ordering::Relaxed),
            pool: RefCell::new(PoolAllocator::default()),
            pool_entries: RefCell::new(Vec::new()),
            roots: Rc::new(RefCell::new(RootList::new())),
            allocation_count: Cell::new(0),
        }
    }

    pub(crate) fn alloc<'gc, T: Trace + Finalize + 'gc>(&'gc self, value: T) -> Gc<'gc, T> {
        let gcbox = GcBox {
            marked: Cell::new(false),
            value,
        };

        let layout = Layout::new::<GcBox<T>>();
        let slot = self
            .pool
            .borrow_mut()
            .try_alloc_bytes(layout)
            .expect("pool allocation failed");

        // SAFETY: slot has correct layout and alignment for GcBox<T>
        unsafe {
            let ptr = slot.cast::<GcBox<T>>();
            ptr.as_ptr().write(gcbox);

            unsafe fn drop_and_free<T: Trace + Finalize>(
                pool: &mut PoolAllocator<'static>,
                ptr: NonNull<u8>,
            ) {
                unsafe {
                    core::ptr::drop_in_place(ptr.cast::<GcBox<T>>().as_ptr());
                    pool.dealloc_bytes(ptr);
                }
            }

            self.pool_entries.borrow_mut().push(PoolEntry {
                ptr: ptr.cast::<u8>(),
                drop_fn: drop_and_free::<T>,
            });

            self.allocation_count.set(self.allocation_count.get() + 1);
            Gc {
                ptr,
                _marker: PhantomData,
            }
        }
    }

    pub(crate) fn collect(&self) {
        let root_count = (*self.roots.borrow()).len();
        println!(
            "Collecting garbage: {} objects, {} roots",
            self.allocation_count.get(),
            root_count
        );
        for ptr in self.roots.borrow().iter_ptrs() {
            unsafe {
                let gcbox = ptr as *mut GcBox<()>;
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
        let mut pool = self.pool.borrow_mut();
        for entry in self.pool_entries.borrow().iter() {
            unsafe {
                (entry.drop_fn)(&mut pool, entry.ptr);
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

    pub fn root<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> Pin<Box<Root<T>>> {
        let collector_roots = Rc::clone(&self.collector.roots);
        let gc_ptr = gc.ptr;

        let root = Box::pin(Root {
            gc_ptr,
            collector_id: self.collector.id,
            collector_roots,
            node: RootListNode {
                ptr: gc_ptr.as_ptr() as *mut u8,
                prev: Cell::new(None),
                next: Cell::new(None),
            },
            _marker: PhantomData,
        });

        unsafe {
            let node_ptr =
                NonNull::new_unchecked(&root.node as *const RootListNode as *mut RootListNode);
            self.collector.roots.borrow().push(node_ptr);
        }

        root
    }

    pub fn collector_id(&self) -> u64 {
        self.collector.id
    }

    pub fn collect(&self) {
        self.collector.collect();
    }
}
