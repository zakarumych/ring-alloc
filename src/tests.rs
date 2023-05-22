#![cfg(not(no_global_oom_handling))]

#[cfg(feature = "alloc")]
mod local {
    use crate::RingAlloc;
    use allocator_api2_tests::make_test;
    make_test![
        test_sizes(RingAlloc::new()),
        test_vec(RingAlloc::new()),
        test_many_boxes(&RingAlloc::new())
    ];
}

#[cfg(feature = "std")]
mod global {
    use crate::OneRingAlloc;

    use allocator_api2::boxed::Box;
    use allocator_api2_tests::make_test;

    make_test![
        test_sizes(OneRingAlloc),
        test_vec(OneRingAlloc),
        test_many_boxes(OneRingAlloc)
    ];

    #[test]
    fn test_global_share() {
        let b = std::thread::spawn(|| Box::new_in(0u32, OneRingAlloc))
            .join()
            .unwrap();
        drop(b);

        drop(Box::new_in(0u32, OneRingAlloc));
    }
}
