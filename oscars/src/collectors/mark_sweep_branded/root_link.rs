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

    /// Returns an iterator over all nodes after `sentinel`.
    pub(crate) fn iter_from_sentinel(
        sentinel: NonNull<Self>,
    ) -> impl Iterator<Item = NonNull<Self>> {
        struct Iter {
            current: Option<NonNull<RootLink>>,
        }

        impl Iterator for Iter {
            type Item = NonNull<RootLink>;

            fn next(&mut self) -> Option<Self::Item> {
                let node = self.current?;
                // SAFETY: nodes are pinned/heap-stable and valid during collection.
                self.current = unsafe { node.as_ref().next.get() };
                Some(node)
            }
        }

        // SAFETY: sentinel is pinned in Collector and outlives the iteration.
        let first = unsafe { sentinel.as_ref().next.get() };
        Iter { current: first }
    }
}
