#[cfg(test)]
mod tests {
    use crate::collectors::mark_sweep_branded::gc::RootNode;
    use crate::collectors::mark_sweep_branded::root_link::RootLink;

    #[test]
    fn root_link_is_thin() {
        // According to the API redesign RFC, RootLink should just be two pointers (prev, next)
        // with no virtual table overhead (no trace_fn fat pointers).
        assert_eq!(
            core::mem::size_of::<RootLink>(),
            core::mem::size_of::<*const ()>() * 2,
            "RootLink must be exactly two words (prev and next pointers)"
        );
    }

    #[test]
    fn root_node_layout_guarantees() {
        // The RFC relies on offset_of!(RootNode<()>, gc_ptr) working exactly due to #[repr(C)]
        // with `link` at offset 0.
        // We verify that the gc_ptr is always immediately following the link, regardless of T.

        let link_size = core::mem::size_of::<RootLink>();

        // Assert offset is correct via offset_of-like macro conceptually
        let make_dummy = || RootNode::<i32> {
            link: RootLink::new(),
            gc_ptr: core::ptr::NonNull::dangling(),
            drop_fn: |_, _| {},
            collector_ptr: core::ptr::null(),
            _marker: core::marker::PhantomData,
        };

        let node = make_dummy();
        let base_ptr = &node as *const _ as usize;
        let link_ptr = &node.link as *const _ as usize;
        let gc_ptr_addr = &node.gc_ptr as *const _ as usize;

        assert_eq!(base_ptr, link_ptr, "RootLink must be at offset 0");
        assert_eq!(
            gc_ptr_addr - base_ptr,
            link_size,
            "gc_ptr must immediately follow RootLink"
        );
    }
}
