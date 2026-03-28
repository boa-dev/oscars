use crate::root_list::RootLink;
use crate::trace::{Finalize, Trace};
use crate::weak::WeakGc;
use core::alloc::Layout;
use core::cell::{Cell, RefCell};
use core::marker::PhantomData;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use oscars::alloc::mempool3::PoolAllocator;
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

/// Pinned root handle that keeps a GC allocation live across `mutate()` boundaries.
///
/// Uses an intrusive linked list. Each root embeds a `RootLink` (prev/next only).
/// The collector walks links at GC time and reads `gc_ptr` via `offset_of!`.
/// `#[repr(C)]` ensures `gc_ptr` is always at offset 0.
///
/// Returned as `Pin<Box<Root<T>>>` so the link address stays stable.
/// `T` must be `Sized` so `gc_ptr` remains a thin pointer, keeping `link` at a
/// fixed offset for type-erased `offset_of!` collection logic.
/// `collector_id` is only for cross-collector misuse detection, not unlinking.
#[must_use = "roots must be kept alive to prevent collection"]
#[repr(C)]
pub struct Root<T: Trace> {
    /// Pointer to the allocated `GcBox<T>`. At offset 0 due to `repr(C)`.
    pub(crate) gc_ptr: NonNull<GcBox<T>>,
    /// ID of the `Collector` that owns this root (for misuse detection).
    pub(crate) collector_id: u64,
    /// Intrusive link (prev/next only, no payload).
    pub(crate) link: RootLink,
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

impl<T: Trace> Drop for Root<T> {
    fn drop(&mut self) {
        // SAFETY: Node address is stable, neighbors outlive this node.
        // Unlinking touches only the embedded prev/next pointers.
        if self.link.is_linked() {
            unsafe {
                RootLink::unlink(NonNull::from(&self.link));
            }
        }
    }
}

// container_of: use offset_of! to get Root<T> from a RootLink pointer.
// Root<T> is repr(C), so gc_ptr is always at offset 0.
// This allows type-erased marking without a Trace bound.

/// Gets gc_ptr from a RootLink pointer.
///
/// # Safety
/// * `link` must point to `Root<T>.link`.
/// * `link_offset` must be `offset_of!(Root<T>, link)`
#[inline(always)]
unsafe fn gc_ptr_from_link(link: NonNull<RootLink>, link_offset: usize) -> *mut u8 {
    let root_ptr = unsafe { (link.as_ptr() as *mut u8).sub(link_offset) };
    // Read the first pointer (gc_ptr at offset 0).
    unsafe { *(root_ptr as *const *mut u8) }
}

struct PoolEntry {
    ptr: NonNull<u8>,
    drop_fn: unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>),
}

/// Owns the sentinel node that heads the intrusive root list.
///
/// ```text
/// sentinel -> root_a.link -> root_b.link -> None
/// ```
///
/// Roots insert after the sentinel on creation and self-unlink on drop (both O(1)).
/// Marking walks the chain and reads `gc_ptr` via `offset_of!`.
pub struct Collector {
    pub(crate) id: u64,
    pool: RefCell<PoolAllocator<'static>>,
    pool_entries: RefCell<Vec<PoolEntry>>,
    /// Pinned sentinel: pure `RootLink`, no payload. Head of the root chain.
    pub(crate) sentinel: Pin<Box<RootLink>>,
    allocation_count: Cell<usize>,
}

impl Collector {
    pub fn new() -> Self {
        Self {
            id: NEXT_COLLECTOR_ID.fetch_add(1, Ordering::Relaxed),
            pool: RefCell::new(PoolAllocator::default()),
            pool_entries: RefCell::new(Vec::new()),
            sentinel: Box::pin(RootLink::new()),
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
        // SAFETY: sentinel is pinned and valid for the lifetime of Self.
        let sentinel_ptr = unsafe {
            NonNull::new_unchecked(
                self.sentinel.as_ref().get_ref() as *const RootLink as *mut RootLink
            )
        };

        // `T: Sized` ensures `gc_ptr` is always a thin (one-word) pointer, so
        // `link` sits at the same byte offset in `Root<T>` for every sized `T`.
        // We instantiate with `i32` as a representative monomorphisation; the
        // resulting offset is correct for any `T: Sized` due to `repr(C)`.
        let link_offset = offset_of!(Root<i32>, link);

        let root_count = RootLink::iter_from_sentinel(sentinel_ptr).count();
        println!(
            "Collecting garbage: {} objects, {} roots",
            self.allocation_count.get(),
            root_count,
        );

        for link_ptr in RootLink::iter_from_sentinel(sentinel_ptr) {
            // SAFETY:
            // * link_ptr points to root.link inside a live Root<T>.
            // * gc_ptr is at offset 0 (repr(C)); read is type-erased.
            // * GcBox.marked is at offset 0 too, so the cast to GcBox<()> is safe.
            unsafe {
                let raw_gc_ptr = gc_ptr_from_link(link_ptr, link_offset);
                let gcbox = raw_gc_ptr as *mut GcBox<()>;
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

impl Default for GcContext {
    fn default() -> Self {
        Self::new()
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

    /// Roots a `Gc<'gc, T>` and returns a `Pin<Box<Root<T>>>`.
    ///
    /// `Pin` is required to keep the link address stable while in the list.
    /// Inserts after the sentinel (O(1)), self-unlinks on drop (O(1)).
    pub fn root<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> Pin<Box<Root<T>>> {
        let gc_ptr = gc.ptr;

        let root = Box::pin(Root {
            gc_ptr,
            collector_id: self.collector.id,
            link: RootLink::new(),
            _marker: PhantomData,
        });

        // SAFETY:
        // * root is pinned: address is stable for its lifetime.
        // * sentinel is pinned in Collector: outlives all roots.
        // * Insertion only touches sentinel.next and root.link.prev/next.
        unsafe {
            let sentinel_ptr = NonNull::new_unchecked(self.collector.sentinel.as_ref().get_ref()
                as *const RootLink
                as *mut RootLink);
            let link_ptr = NonNull::from(&root.link);
            RootLink::link_after(sentinel_ptr, link_ptr);
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
