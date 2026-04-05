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
/// Uses an intrusive linked list. `#[repr(C)]` with `link` first allows
/// casting `*mut RootLink` directly to `*mut RootNode<T>` without pointer math.
///
/// `Root<'id, T>` wraps a raw pointer to keep the list link stable in memory
/// and avoid moving `Box`, preventing Stacked Borrows aliasing UB.
/// `T: Sized` ensures `gc_ptr` is a single word thin pointer, making
/// type-erased `*mut u8` collector reads sound
///
/// The invariant `'id` lifetime ties this root to a specific `GcContext<'id>`,
/// preventing cross-context usage at compile time.
#[must_use = "roots must be kept alive to prevent collection"]
pub struct Root<'id, T: Trace> {
    raw: NonNull<RootNode<'id, T>>,
}

#[repr(C)]
pub(crate) struct RootNode<'id, T: Trace> {
    /// Intrusive list node. Placed at offset 0 for direct base pointer casting.
    pub(crate) link: RootLink,
    /// GC allocation pointer. `T: Sized` ensures this is a thin pointer
    pub(crate) gc_ptr: NonNull<GcBox<T>>,
    /// Invariant lifetime marker tying this root to a specific `GcContext`.
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace> Root<'id, T> {
    /// Converts this root back into a `Gc<'gc, T>` within a mutation context.
    ///
    /// The `'id` lifetime on `Root` must match the `'id` lifetime on `MutationContext`.
    pub fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Gc<'gc, T> {
        Gc {
            ptr: unsafe { self.raw.as_ref().gc_ptr },
            _marker: PhantomData,
        }
    }
}

impl<'id, T: Trace> Drop for Root<'id, T> {
    fn drop(&mut self) {
        // SAFETY: Node address is stable, neighbors outlive this node.
        // Unlinking touches only the embedded prev/next pointers.
        unsafe {
            let node = Box::from_raw(self.raw.as_ptr());
            if node.link.is_linked() {
                RootLink::unlink(NonNull::from(&node.link));
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
    pool: RefCell<PoolAllocator<'static>>,
    pool_entries: RefCell<Vec<PoolEntry>>,
    /// Pinned sentinel: pure `RootLink`, no payload. Head of the root chain.
    pub(crate) sentinel: Pin<Box<RootLink>>,
    allocation_count: Cell<usize>,
}

impl Collector {
    pub fn new() -> Self {
        Self {
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
        let gc_ptr_offset = offset_of!(RootNode<i32>, gc_ptr);
        // `#[repr(C)]` + `T: Sized` guarantees `gc_ptr` is always a single
        // pointer-width field at the same offset regardless of `T`.  Verify
        // this with a second concrete type so any future repr change is caught
        // immediately rather than silently corrupting the root traversal.
        debug_assert_eq!(
            gc_ptr_offset,
            offset_of!(RootNode<u64>, gc_ptr),
            "RootNode<T> layout must be identical for all T: Sized"
        );

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

/// The main GC context that owns a collector.
///
/// The invariant `'id` lifetime uniquely identifies this context, ensuring
/// `Root<'id, T>` can only be used with its corresponding `MutationContext`.
pub struct GcContext<'id> {
    collector: Collector,
    /// Invariant lifetime marker. Using `*mut &'id ()` makes it invariant.
    _marker: PhantomData<*mut &'id ()>,
}

/// Creates a new GC context and passes it to the closure.
///
/// The `for<'id>` bound creates a fresh `'id` for each context,
/// preventing cross-context usage of `Root` or `WeakGc`.
///
/// # Example
/// ```ignore
/// with_gc(|ctx| {
///     ctx.mutate(|cx| {
///         let v = cx.alloc(42i32);
///         let root = cx.root(v);
///         assert_eq!(*root.get(cx).get(), 42);
///     });
/// });
/// ```
pub fn with_gc<R, F: for<'id> FnOnce(GcContext<'id>) -> R>(f: F) -> R {
    f(GcContext {
        collector: Collector::new(),
        _marker: PhantomData,
    })
}

impl<'id> GcContext<'id> {
    /// Runs a mutation closure with access to GC operations.
    ///
    /// The `'gc` lifetime brands pointers to this phase, while `'id` ties them to this context.
    pub fn mutate<R>(&self, f: impl for<'gc> FnOnce(&MutationContext<'id, 'gc>) -> R) -> R {
        let cx = MutationContext {
            collector: &self.collector,
            _marker: PhantomData,
        };
        f(&cx)
    }
}

/// Context for GC mutations within a `mutate()` closure.
///
/// `'id` ties this context to a specific `GcContext<'id>` (invariant lifetime).
/// `'gc` brands all `Gc<'gc, T>` pointers to this mutation phase.
pub struct MutationContext<'id, 'gc> {
    pub(crate) collector: &'gc Collector,
    /// Invariant lifetime marker for context identity.
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, 'gc> MutationContext<'id, 'gc> {
    pub fn alloc<T: Trace + Finalize + 'gc>(&self, value: T) -> Gc<'gc, T> {
        self.collector.alloc(value)
    }

    pub fn alloc_weak<T: Trace + Finalize + 'gc>(&self, value: T) -> WeakGc<'id, T> {
        let gc = self.alloc(value);
        WeakGc {
            ptr: gc.ptr,
            _marker: PhantomData,
        }
    }

    /// Roots a `Gc<'gc, T>` and returns an opaque `Root<'id, T>` handle.
    ///
    /// The returned `Root` is tied to this specific `GcContext<'id>` at compile time.
    ///
    /// The returned handle encapsulates a dynamically allocated linked list node
    /// whose address remains stable. Inserts after the sentinel (O(1)), self-unlinks on drop (O(1)).
    pub fn root<T: Trace + Finalize + 'gc>(&self, gc: Gc<'gc, T>) -> Root<'id, T> {
        let gc_ptr = gc.ptr;

        let node = Box::new(RootNode {
            link: RootLink::new(),
            gc_ptr,
            _marker: PhantomData,
        });

        let raw = unsafe { NonNull::new_unchecked(Box::into_raw(node)) };

        // SAFETY:
        // * root pointer address is stable for its lifetime.
        // * sentinel is pinned in Collector: outlives all roots.
        // * Insertion only touches sentinel.next and root.link.prev/next.
        unsafe {
            let sentinel_ptr = NonNull::new_unchecked(self.collector.sentinel.as_ref().get_ref()
                as *const RootLink
                as *mut RootLink);
            let link_ptr = raw.cast::<RootLink>();
            RootLink::link_after(sentinel_ptr, link_ptr);
        }

        Root { raw }
    }

    pub fn collect(&self) {
        self.collector.collect();
    }
}
