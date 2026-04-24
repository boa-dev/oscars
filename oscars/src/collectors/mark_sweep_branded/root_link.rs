//! Intrusive singly-linked root list.

use core::cell::Cell;
use core::ptr::NonNull;

/// Intrusive link node
pub(crate) struct RootLink {
    prev: Cell<Option<NonNull<RootLink>>>,
    next: Cell<Option<NonNull<RootLink>>>,
}

impl RootLink {
    /// Creates a new, unlinked node.
    pub(crate) const fn new() -> Self {
        Self {
            prev: Cell::new(None),
            next: Cell::new(None),
        }
    }

    /// Returns `true` if this node is currently part of a list.
    #[inline]
    pub(crate) fn is_linked(&self) -> bool {
        self.prev.get().is_some()
    }

    /// Inserts `node` immediately after `anchor`.
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

/// Iterator over root links in the intrusive list.
pub(crate) struct RootLinkIter {
    current: Option<NonNull<RootLink>>,
}

impl Iterator for RootLinkIter {
    type Item = NonNull<RootLink>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.current?;
        // SAFETY: nodes are pinned/heap-stable and valid during collection.
        self.current = unsafe { node.as_ref().next.get() };
        Some(node)
    }
}

/// Sentinel node for the root list.
#[repr(transparent)]
pub(crate) struct RootSentinel(core::pin::Pin<rust_alloc::boxed::Box<RootLink>>);

impl RootSentinel {
    /// Creates a new sentinel node
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
