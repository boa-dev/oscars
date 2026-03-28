use core::cell::Cell;
use core::ptr::NonNull;

/// Intrusive linked list node for O(1) root removal
pub(crate) struct RootListNode {
    pub(crate) ptr: *mut u8,
    pub(crate) prev: Cell<Option<NonNull<RootListNode>>>,
    pub(crate) next: Cell<Option<NonNull<RootListNode>>>,
}

/// Intrusive doubly linked list of roots
pub(crate) struct RootList {
    head: Cell<Option<NonNull<RootListNode>>>,
    tail: Cell<Option<NonNull<RootListNode>>>,
    len: Cell<usize>,
}

impl RootList {
    pub(crate) fn new() -> Self {
        Self {
            head: Cell::new(None),
            tail: Cell::new(None),
            len: Cell::new(0),
        }
    }

    /// # Safety
    /// Node must remain pinned until removed
    pub(crate) unsafe fn push(&self, node: NonNull<RootListNode>) {
        unsafe {
            let node_ref = node.as_ref();
            node_ref.prev.set(self.tail.get());
            node_ref.next.set(None);

            if let Some(tail) = self.tail.get() {
                tail.as_ref().next.set(Some(node));
            } else {
                self.head.set(Some(node));
            }
            self.tail.set(Some(node));
            self.len.set(self.len.get() + 1);
        }
    }

    /// # Safety
    /// Node must be in this list
    pub(crate) unsafe fn remove(&self, node: NonNull<RootListNode>) {
        unsafe {
            let node_ref = node.as_ref();
            let prev = node_ref.prev.get();
            let next = node_ref.next.get();

            match prev {
                Some(p) => p.as_ref().next.set(next),
                None => self.head.set(next),
            }

            match next {
                Some(n) => n.as_ref().prev.set(prev),
                None => self.tail.set(prev),
            }

            self.len.set(self.len.get() - 1);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len.get()
    }

    pub(crate) fn iter_ptrs(&self) -> impl Iterator<Item = *mut u8> + '_ {
        struct Iter {
            current: Option<NonNull<RootListNode>>,
        }

        impl Iterator for Iter {
            type Item = *mut u8;

            fn next(&mut self) -> Option<Self::Item> {
                let node = self.current?;
                unsafe {
                    let node_ref = node.as_ref();
                    self.current = node_ref.next.get();
                    Some(node_ref.ptr)
                }
            }
        }

        Iter {
            current: self.head.get(),
        }
    }
}
