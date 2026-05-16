use crate::{
    alloc::mempool3::{PoolAllocator, PoolPointer},
    collectors::mark_sweep_branded::{
        gc::Gc, gc_box::GcBox, mutation_ctx::MutationContext, trace::Trace,
    },
};
use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::NonNull;

pub(crate) type RootDropFn = unsafe fn(&mut PoolAllocator<'static>, NonNull<u8>);

/// Intrusive link node
pub(crate) struct RootLink {
    prev: Cell<Option<NonNull<RootLink>>>,
    next: Cell<Option<NonNull<RootLink>>>,
}

impl RootLink {
    pub(crate) const fn new() -> Self {
        Self {
            prev: Cell::new(None),
            next: Cell::new(None),
        }
    }

    /// Returns true if this node is currently part of a list
    #[inline]
    pub(crate) fn is_linked(&self) -> bool {
        self.prev.get().is_some()
    }

    /// Inserts `node` immediately after `anchor`
    ///
    /// # Safety
    ///
    /// Both `anchor` and `node` must remain at stable addresses until unlinked.
    pub(crate) unsafe fn link_after(anchor: NonNull<Self>, node: NonNull<Self>) {
        unsafe {
            let anchor_ref = anchor.as_ref();
            let node_ref = node.as_ref();
            let old_next = anchor_ref.next.get();

            node_ref.prev.set(Some(anchor));
            node_ref.next.set(old_next);
            anchor_ref.next.set(Some(node));

            if let Some(next) = old_next {
                next.as_ref().prev.set(Some(node));
            }
        }
    }

    /// Removes `node` from the list.
    ///
    /// # Safety
    ///
    /// `node` must currently be linked.
    pub(crate) unsafe fn unlink(node: NonNull<Self>) {
        unsafe {
            let node_ref = node.as_ref();
            let prev = node_ref.prev.get();
            let next = node_ref.next.get();

            if let Some(p) = prev {
                p.as_ref().next.set(next);
            }
            if let Some(n) = next {
                n.as_ref().prev.set(prev);
            }

            node_ref.prev.set(None);
            node_ref.next.set(None);
        }
    }
}

pub(crate) struct RootLinkIter {
    current: Option<NonNull<RootLink>>,
}

impl Iterator for RootLinkIter {
    type Item = NonNull<RootLink>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.current?;
        // SAFETY: nodes are pinned and valid during collection.
        self.current = unsafe { node.as_ref().next.get() };
        Some(node)
    }
}

/// Sentinel node for the root list
#[repr(transparent)]
pub(crate) struct RootSentinel(core::pin::Pin<rust_alloc::boxed::Box<RootLink>>);

impl RootSentinel {
    pub(crate) fn new() -> Self {
        Self(rust_alloc::boxed::Box::pin(RootLink::new()))
    }

    /// Returns a pointer to the underlying RootLink
    pub(crate) fn as_ptr(&self) -> NonNull<RootLink> {
        // SAFETY: The sentinel is pinned and the pointer is derived from a valid Box.
        unsafe {
            NonNull::new_unchecked(self.0.as_ref().get_ref() as *const RootLink as *mut RootLink)
        }
    }

    /// Returns an iterator over all root nodes after this sentinel.
    pub(crate) fn iter(&self) -> RootLinkIter {
        let first = self.0.as_ref().next.get();
        RootLinkIter { current: first }
    }
}

/// Heap node backing a [`Root`]
#[repr(C)]
pub(crate) struct RootNode<'id, T: Trace> {
    pub(crate) link: RootLink,
    /// Pointer to the allocation
    pub(crate) gc_ptr: PoolPointer<'static, GcBox<T>>,
    /// Type-erased drop function for freeing this RootNode
    pub(crate) drop_fn: RootDropFn,
    /// Raw pointer to the Collector for freeing this node
    pub(crate) collector_ptr: *const crate::collectors::mark_sweep_branded::Collector,
    pub(crate) _marker: PhantomData<*mut &'id ()>,
}

impl<'id, T: Trace> RootNode<'id, T> {
    /// Creates a new [`RootNode`] initialised for the given `gc_ptr` and `collector`
    pub(crate) fn new_in(
        gc_ptr: PoolPointer<'static, GcBox<T>>,
        collector: &crate::collectors::mark_sweep_branded::Collector,
    ) -> Self {
        unsafe fn drop_and_free<T: Trace>(pool: &mut PoolAllocator<'static>, ptr: NonNull<u8>) {
            use crate::alloc::mempool3::PoolItem;
            unsafe {
                let typed_ptr = ptr.cast::<PoolItem<RootNode<'_, T>>>();
                core::ptr::drop_in_place(typed_ptr.as_ptr());
                pool.free_slot(ptr);
            }
        }

        Self {
            link: RootLink::new(),
            gc_ptr,
            drop_fn: drop_and_free::<T>,
            collector_ptr: collector as *const _,
            _marker: PhantomData,
        }
    }
}

/// Type-erased version of [`RootNode`] for use during collection.
///
/// Since [`RootNode`] is `repr(C)` and `link` is always the first field,
/// a `NonNull<RootLink>` from the sentinel iterator can be safely cast to
/// `NonNull<ErasedRootNode>` to read `gc_ptr` without knowing `T`.
#[repr(C)]
pub(crate) struct ErasedRootNode {
    pub(crate) link: RootLink,
    pub(crate) gc_ptr: PoolPointer<'static, GcBox<()>>,
}

/// A handle that keeps a GC allocation live.
#[must_use = "dropping a root unregisters it from the GC"]
pub struct Root<'id, T: Trace> {
    pub(crate) raw: NonNull<RootNode<'id, T>>,
}

impl<'id, T: Trace> Root<'id, T> {
    /// Converts this root into a `Gc` pointer
    pub fn get<'gc>(&self, _cx: &MutationContext<'id, 'gc>) -> Gc<'gc, T> {
        Gc {
            // SAFETY: `raw` is non null and valid
            ptr: unsafe { self.raw.as_ref().gc_ptr },
            _marker: PhantomData,
        }
    }
}

impl<'id, T: Trace> Drop for Root<'id, T> {
    fn drop(&mut self) {
        unsafe {
            let node_ref = self.raw.as_ref();
            if node_ref.link.is_linked() {
                RootLink::unlink(NonNull::from(&node_ref.link));
            }
            // SAFETY: collector_ptr is valid for the lifetime of the GcContext
            let collector = &*node_ref.collector_ptr;
            collector.free_root_node(self.raw.cast::<u8>(), node_ref.drop_fn);
        }
    }
}
