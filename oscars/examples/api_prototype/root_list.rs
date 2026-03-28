use core::cell::Cell;
use core::ptr::NonNull;

/// Intrusive link node, analogous to `intrusive_collections::LinkedListLink
///
/// Contains only `prev`/`next` pointers. Embed inside a struct (e.g. `Root<T>`)
/// and recover the container via `offset_of!`
///
/// Must not move while linked. Callers enforce this with `Pin`
pub(crate) struct RootLink {
    prev: Cell<Option<NonNull<RootLink>>>,
    next: Cell<Option<NonNull<RootLink>>>,
}

impl RootLink {
    /// Creates a new unlinked node. `const fn` so it can be used in pinned sentinels.
    pub(crate) const fn new() -> Self {
        Self {
            prev: Cell::new(None),
            next: Cell::new(None),
        }
    }

    /// Returns `true` if this node is currently in a list.
    /// Uses `prev.is_some()` as the indicator; `unlink` clears it
    #[inline]
    pub(crate) fn is_linked(&self) -> bool {
        self.prev.get().is_some()
    }

    /// Inserts `node` immediately after `anchor`
    ///
    /// # Safety
    /// Both `anchor` and `node` must be pinned until unlinked.
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

    /// Removes the node from the list in O(1). Sets `is_linked()` to false.
    ///
    /// # Safety
    /// `node` must currently be linked.
    pub(crate) unsafe fn unlink(node: NonNull<Self>) {
        unsafe {
            let node_ref = node.as_ref();
            let prev = node_ref.prev.get();
            let next = node_ref.next.get();

            // Re-wire neighbours around this node.
            if let Some(p) = prev {
                p.as_ref().next.set(next);
            }
            if let Some(n) = next {
                n.as_ref().prev.set(prev);
            }

            // Clear to make is_linked() == false and catch double-unlink bugs.
            node_ref.prev.set(None);
            node_ref.next.set(None);
        }
    }

    /// Iterates all nodes after the sentinel. Skips the sentinel itself.
    /// Caller uses `offset_of!` to get the `Root<T>` from each yielded link.
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
                // SAFETY: nodes are pinned and valid during iteration.
                unsafe {
                    self.current = node.as_ref().next.get();
                }
                Some(node)
            }
        }

        // SAFETY: sentinel is pinned and owned by Collector.
        let first = unsafe { sentinel.as_ref().next.get() };
        Iter { current: first }
    }
}
