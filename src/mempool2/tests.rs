
use super::Pool;

#[test]
fn basic_alloc() {
    // TODO: Is there a better way to initialize a page?
    //
    // Maybe have the page be time N * chunk_size 
    let mut allocator = Pool::init(size_of::<usize>(), 256, align_of::<usize>()).unwrap();

    for i in 0..(256 / size_of::<usize>()) {
        let _ = allocator.alloc(i);
    }
    assert!(allocator.try_alloc(0).is_err());
}

#[test]
fn alloc_dealloc_realloc() {
    #[derive(Debug)]
    struct Item {
        one: usize,
        phase: usize,
    }

    impl Drop for Item {
        fn drop(&mut self) {}
    }

    let mut allocator = Pool::init(
        size_of::<Item>(),
        4096,
        align_of::<Item>(),
    ).unwrap();

    let mut collection = alloc::vec::Vec::default();
    // Fill all of our chunks
    for i in (0..4096).step_by(size_of::<Item>()) {
        let allocated = allocator.try_alloc(Item {
            one: i,
            phase: 1
        }).unwrap();
        collection.push(allocated);
    }

    assert!(allocator.try_alloc(Item { one: 0, phase: 0}).is_err());

    let mut still_allocated = alloc::vec::Vec::default();
    for item in collection {
        let item_ref = unsafe { item.as_ref() };
        // Deallocate any item divisble by 32, but leave those
        // divisible by 16 still allocated.
        if item_ref.one % 32 == 0 {
            unsafe { allocator.dealloc(item) };
        } else {
            still_allocated.push(item)
        }
    }

    let mut reallocated = alloc::vec::Vec::default();
    for i in (0usize..4096).step_by(size_of::<Item>() * 2) {
        let allocated = allocator.try_alloc(Item {
            one: i + size_of::<Item>(),
            phase: 2
        }).unwrap();
        reallocated.push(allocated)
    }

    for (first_phase_ptr, second_phase_ptr) in still_allocated.iter().zip(&reallocated) {
        let first_phase = unsafe { first_phase_ptr.as_ref() };
        let second_phase = unsafe { second_phase_ptr.as_ref() };

        assert_ne!(first_phase_ptr, second_phase_ptr);

        // Assert that the first phase number is 15 below the second.
        //
        // We check this to ensure that we overright the previous value
        // which would've been one step up
        assert_eq!(first_phase.one, second_phase.one);
        assert_eq!(first_phase.phase, 1);
        assert_eq!(second_phase.phase, 2);
    }
}

#[test]
fn drop() {
    use alloc::rc::Rc;
    use core::sync::atomic::{AtomicBool, Ordering};

    struct MyS {
        dropped: Rc<AtomicBool>,
    }

    impl Drop for MyS {
        fn drop(&mut self) {
            self.dropped
                .store(true, Ordering::SeqCst);
        }
    }

    let mut pool = Pool::init(
        size_of::<MyS>(),
        4096,
        align_of::<MyS>(),
    ).unwrap();


    let dropped = Rc::new(AtomicBool::new(false));
    let a = pool.alloc(MyS {
        dropped: dropped.clone(),
    });

    unsafe { pool.dealloc(a) };
    assert!(dropped.load(Ordering::SeqCst));
}

