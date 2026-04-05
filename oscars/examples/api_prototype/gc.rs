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

/// Lifetime-branded root handle tied to a single mutation context.
///
/// Unlike `Root<T>`, this cannot escape the `'gc` mutation lifetime and
/// therefore cannot be used with another collector/context in safe code.
#[must_use = "scoped roots must be used within the active mutation context"]
pub struct ScopedRoot<'gc, T: Trace + ?Sized + 'gc> {
    gc: Gc<'gc, T>,
}

impl<'gc, T: Trace + ?Sized + 'gc> ScopedRoot<'gc, T> {
    pub fn get(&self, _cx: &MutationContext<'gc>) -> Gc<'gc, T> {
        self.gc
    }
}

/// Pinned root handle that keeps a GC allocation live across `mutate()` boundaries.
///
/// Uses an intrusive linked list. `#[repr(C)]` with `link` first allows
/// casting `*mut RootLink` directly to `*mut Root<T>` without pointer math.
///
/// `Pin<Box<Root<T>>>` keeps the list link stable in memory.
/// `T: Sized` ensures `gc_ptr` is a single word thin pointer, making
/// type-erased `*mut u8` collector reads sound
#[must_use = "roots must be kept alive to prevent collection"]
#[repr(C)]
pub struct Root<T: Trace> {
    /// Intrusive list node. Placed at offset 0 for direct base pointer casting.
    pub(crate) link: RootLink,
    /// GC allocation pointer. `T: Sized` ensures this is a thin pointer
    pub(crate) gc_ptr: NonNull<GcBox<T>>,
    /// ID of the `Collector` that owns this root (for misuse detection).
    pub(crate) collector_id: u64,
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

// `link` is at offset 0, so its pointer represents the entire `Root<T>`.
// To find `gc_ptr`, we just add its expected offset. `T: Sized` ensures
// `gc_ptr` is a thin pointer, making the raw `*mut u8` read safe.

/// Gets gc_ptr from a RootLink pointer without knowing the generic type `T`
///
/// # Safety
/// * `link` must point to a real Root<T> for some `T: Trace + Sized`
/// * `gc_ptr_offset` must be the exact distance to `gc_ptr` for that `T`
#[inline(always)]
unsafe fn gc_ptr_from_link(link: NonNull<RootLink>, gc_ptr_offset: usize) -> *mut u8 {
    // Both point to the exact same memory address
    let gc_ptr_field = unsafe { (link.as_ptr() as *mut u8).add(gc_ptr_offset) };
    unsafe { *(gc_ptr_field as *const *mut u8) }
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

        // `link` is at offset 0, so a pointer to `link` is a pointer to the start of the struct.
        // `gc_ptr` comes right after it. Because `T` is `Sized`, the distance to `gc_ptr`
        // is exactly the same no matter what generic type `T` is. We use `Root<i32>` as a
        // dummy type just to calculate this fixed offset.
        let gc_ptr_offset = offset_of!(Root<i32>, gc_ptr);

        let root_count = RootLink::iter_from_sentinel(sentinel_ptr).count();
        println!(
            "Collecting garbage: {} objects, {} roots",
            self.allocation_count.get(),
            root_count,
        );

        for link_ptr in RootLink::iter_from_sentinel(sentinel_ptr) {
            // SAFETY:
            // * link_ptr points to root.link inside a live Root<T>.
            // * link is at offset 0 (repr(C)), so link_ptr == root_ptr.
            // * gc_ptr is thin (T: Sized); the *mut u8 read is sound.
            // * GcBox.marked is at offset 0, so the cast to GcBox<()> is safe.
            unsafe {
                let raw_gc_ptr = gc_ptr_from_link(link_ptr, gc_ptr_offset);
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
            link: RootLink::new(),
            gc_ptr,
            collector_id: self.collector.id,
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

    /// Creates a root that is statically bound to this mutation lifetime.
    ///
    /// This is a prototype path to evaluate whether root/context pairing can be
    /// fully enforced at compile time. The handle cannot escape `'gc`.
    pub fn root_scoped<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> ScopedRoot<'gc, T> {
        ScopedRoot { gc }
    }

    pub fn collector_id(&self) -> u64 {
        self.collector.id
    }

    pub fn collect(&self) {
        self.collector.collect();
    }
}
