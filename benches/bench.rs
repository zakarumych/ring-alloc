#![cfg_attr(feature = "nightly", feature(allocator_api))]

use core::ptr::NonNull;

use allocator_api2::{
    alloc::{AllocError, Allocator, Global, Layout},
    boxed::Box,
    vec::Vec,
};

use criterion::*;
use ring_alloc::*;

#[repr(transparent)]
struct Bump<'a> {
    bump: &'a mut bumpalo::Bump,
}

impl Bump<'_> {
    #[inline(always)]
    fn reset(&mut self) {
        self.bump.reset();
    }
}

unsafe impl<'a> Allocator for Bump<'a> {
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate(&&*self.bump, layout)
    }

    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        Allocator::deallocate(&&*self.bump, ptr, layout)
    }
}

#[repr(transparent)]
struct BlinkAlloc<'a> {
    blink: &'a mut blink_alloc::BlinkAlloc,
}

impl BlinkAlloc<'_> {
    #[inline(always)]
    fn reset(&mut self) {
        self.blink.reset();
    }
}

unsafe impl<'a> Allocator for BlinkAlloc<'a> {
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        Allocator::allocate(&&*self.blink, layout)
    }

    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        Allocator::deallocate(&&*self.blink, ptr, layout)
    }
}

/// GlobalAlloc that counts the number of allocations and deallocations
/// and number of bytes allocated and deallocated.
#[cfg(feature = "bench-with-counting-allocator")]
struct CountingGlobalAlloc {
    allocations: AtomicUsize,
    deallocations: AtomicUsize,

    bytes_allocated: AtomicUsize,
    bytes_deallocated: AtomicUsize,
}

#[cfg(feature = "bench-with-counting-allocator")]
impl CountingGlobalAlloc {
    pub const fn new() -> Self {
        CountingGlobalAlloc {
            allocations: AtomicUsize::new(0),
            deallocations: AtomicUsize::new(0),
            bytes_allocated: AtomicUsize::new(0),
            bytes_deallocated: AtomicUsize::new(0),
        }
    }

    pub fn reset_stat(&self) {
        self.allocations.store(0, Ordering::Relaxed);
        self.deallocations.store(0, Ordering::Relaxed);
        self.bytes_allocated.store(0, Ordering::Relaxed);
        self.bytes_deallocated.store(0, Ordering::Relaxed);
    }

    pub fn print_stat(&self) {
        let allocations = self.allocations.load(Ordering::Relaxed);
        let deallocations = self.deallocations.load(Ordering::Relaxed);
        let bytes_allocated = self.bytes_allocated.load(Ordering::Relaxed);
        let bytes_deallocated = self.bytes_deallocated.load(Ordering::Relaxed);

        eprintln!(
            "allocations: {allocations},
            deallocations: {deallocations},
            bytes_allocated: {bytes_allocated},
            bytes_deallocated: {bytes_deallocated}"
        )
    }
}

#[cfg(feature = "bench-with-counting-allocator")]
unsafe impl core::alloc::GlobalAlloc for CountingGlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            self.allocations.fetch_add(1, Ordering::Relaxed);
            self.bytes_allocated
                .fetch_add(layout.size(), Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        self.deallocations.fetch_add(1, Ordering::Relaxed);
        self.bytes_deallocated
            .fetch_add(layout.size(), Ordering::Relaxed);
    }
}

#[cfg(feature = "bench-with-counting-allocator")]
#[global_allocator]
static COUNTING_ALLOCATOR: CountingGlobalAlloc = CountingGlobalAlloc::new();

#[inline(always)]
fn print_mem_stat() {
    #[cfg(feature = "bench-with-counting-allocator")]
    COUNTING_ALLOCATOR.print_stat();
}

#[inline(always)]
fn reset_mem_stat() {
    #[cfg(feature = "bench-with-counting-allocator")]
    COUNTING_ALLOCATOR.reset_stat();
}

const WARM_UP_SIZE: usize = 65535;
const VEC_SIZES: [usize; 4] = [10, 146, 2134, 17453];

fn bench_alloc<A>(
    name: &str,
    c: &mut Criterion,
    mut alloc: A,
    reset: impl Fn(&mut A),
    shrink_larger_align: bool,
) where
    A: Allocator,
{
    let mut group = c.benchmark_group(format!("allocation/{name}"));

    reset_mem_stat();

    group.bench_function(format!("alloc"), |b| {
        b.iter(|| {
            let ptr = black_box(alloc.allocate(Layout::new::<u32>()).unwrap());
            unsafe {
                alloc.deallocate(ptr.cast(), Layout::new::<u32>());
            }
        });
        reset(&mut alloc);
    });

    print_mem_stat();
    // reset_mem_stat();

    // group.bench_function(format!("grow same align x {SIZE}"), |b| {
    //     b.iter(|| {
    //         for _ in 0..SIZE {
    //             unsafe {
    //                 let ptr = alloc.allocate(Layout::new::<u32>()).unwrap();
    //                 let ptr = alloc
    //                     .grow(ptr.cast(), Layout::new::<u32>(), Layout::new::<[u32; 2]>())
    //                     .unwrap();
    //                 black_box(ptr);
    //             }
    //         }
    //         reset(&mut alloc);
    //     })
    // });

    // group.bench_function(format!("grow smaller align x {SIZE}"), |b| {
    //     b.iter(|| {
    //         for _ in 0..SIZE {
    //             unsafe {
    //                 let ptr = alloc.allocate(Layout::new::<u32>()).unwrap();
    //                 let ptr = alloc
    //                     .grow(ptr.cast(), Layout::new::<u32>(), Layout::new::<[u16; 4]>())
    //                     .unwrap();
    //                 let ptr = black_box(ptr);
    //                 alloc.deallocate(ptr.cast(), Layout::new::<[u16; 4]>());
    //             }
    //         }
    //         reset(&mut alloc);
    //     })
    // });

    // group.bench_function(format!("grow larger align x {SIZE}"), |b| {
    //     b.iter(|| {
    //         for _ in 0..SIZE {
    //             unsafe {
    //                 let ptr = alloc.allocate(Layout::new::<u32>()).unwrap();
    //                 let ptr = alloc
    //                     .grow(ptr.cast(), Layout::new::<u32>(), Layout::new::<u64>())
    //                     .unwrap();
    //                 let ptr = black_box(ptr);
    //                 alloc.deallocate(ptr.cast(), Layout::new::<u64>());
    //             }
    //         }
    //         reset(&mut alloc);
    //     })
    // });

    // group.bench_function(format!("shrink same align x {SIZE}"), |b| {
    //     b.iter(|| {
    //         for _ in 0..SIZE {
    //             unsafe {
    //                 let ptr = alloc.allocate(Layout::new::<[u32; 2]>()).unwrap();
    //                 let ptr = alloc
    //                     .shrink(ptr.cast(), Layout::new::<[u32; 2]>(), Layout::new::<u32>())
    //                     .unwrap();
    //                 let ptr = black_box(ptr);
    //                 alloc.deallocate(ptr.cast(), Layout::new::<u32>());
    //             }
    //         }
    //         reset(&mut alloc);
    //     })
    // });

    // group.bench_function(format!("shrink smaller align x {SIZE}"), |b| {
    //     b.iter(|| {
    //         for _ in 0..SIZE {
    //             unsafe {
    //                 let ptr = alloc.allocate(Layout::new::<u32>()).unwrap();
    //                 let ptr = alloc
    //                     .shrink(ptr.cast(), Layout::new::<u32>(), Layout::new::<u16>())
    //                     .unwrap();
    //                 let ptr = black_box(ptr);
    //                 alloc.deallocate(ptr.cast(), Layout::new::<u16>());
    //             }
    //         }
    //         reset(&mut alloc);
    //     })
    // });

    // if shrink_larger_align {
    //     group.bench_function(format!("shrink larger align x {SIZE}"), |b| {
    //         b.iter(|| {
    //             for _ in 0..SIZE {
    //                 unsafe {
    //                     let ptr = alloc.allocate(Layout::new::<[u32; 4]>()).unwrap();
    //                     let ptr = alloc
    //                         .shrink(ptr.cast(), Layout::new::<[u32; 4]>(), Layout::new::<u64>())
    //                         .unwrap();
    //                     let ptr = black_box(ptr);
    //                     alloc.deallocate(ptr.cast(), Layout::new::<u64>());
    //                 }
    //             }
    //             reset(&mut alloc);
    //         })
    //     });
    // }

    // print_mem_stat();

    group.finish();
}

fn bench_warm_up<A>(name: &str, c: &mut Criterion, mut alloc: A, mut reset: impl FnMut(&mut A))
where
    A: Allocator,
{
    let mut group = c.benchmark_group(format!("warm-up/{name}"));

    reset_mem_stat();

    group.bench_function(format!("alloc 4 bytes x {WARM_UP_SIZE}"), |b| {
        b.iter(|| {
            for _ in 0..WARM_UP_SIZE {
                black_box(alloc.allocate(Layout::new::<u32>()).unwrap());
            }
            reset(&mut alloc);
        })
    });

    print_mem_stat();
    group.finish();
}

fn bench_vec<A>(name: &str, c: &mut Criterion, mut alloc: A, reset: impl Fn(&mut A))
where
    A: Allocator,
{
    let mut group = c.benchmark_group(format!("vec/{name}"));

    reset_mem_stat();

    for size in VEC_SIZES {
        group.bench_function(format!("push x {size}"), |b| {
            b.iter(|| {
                let mut vec = Vec::new_in(&alloc);
                for i in 0..size {
                    vec.push(i);
                }
                drop(vec);
                reset(&mut alloc);
            })
        });

        print_mem_stat();
        reset_mem_stat();

        group.bench_function(format!("reserve_exact(1) x {size}"), |b| {
            b.iter(|| {
                let mut vec = Vec::<u32, _>::new_in(&alloc);
                for i in 0..size {
                    vec.reserve_exact(i);
                }
                drop(vec);
                reset(&mut alloc);
            })
        });
    }

    print_mem_stat();
    reset_mem_stat();

    group.finish();
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut ring_alloc = RingAlloc::new();
    let mut bump = bumpalo::Bump::new();
    let mut blink = blink_alloc::BlinkAlloc::new();

    bench_warm_up("Global", c, Global, |_| {});

    bench_warm_up("ring_alloc::RingAlloc", c, ring_alloc.clone(), |ra| {
        ring_alloc = RingAlloc::new();
        *ra = ring_alloc.clone();
    });

    #[cfg(feature = "std")]
    bench_warm_up("ring_alloc::OneRingAlloc", c, OneRingAlloc, |_| {});

    bench_warm_up("bumpalo::Bump", c, Bump { bump: &mut bump }, |bump| {
        *bump.bump = bumpalo::Bump::new()
    });

    bench_warm_up(
        "blink_alloc::BlinkAlloc",
        c,
        BlinkAlloc { blink: &mut blink },
        |blink| *blink.blink = blink_alloc::BlinkAlloc::new(),
    );

    bench_alloc("Global", c, Global, |_| {}, true);

    bench_alloc("ring_alloc::RingAlloc", c, ring_alloc.clone(), |_| {}, true);

    #[cfg(feature = "std")]
    bench_alloc("ring_alloc::OneRingAlloc", c, OneRingAlloc, |_| {}, true);

    bench_alloc(
        "bumpalo::Bump",
        c,
        Bump { bump: &mut bump },
        |b| b.reset(),
        false,
    );

    bench_alloc(
        "blink_alloc::BlinkAlloc",
        c,
        BlinkAlloc { blink: &mut blink },
        |b| b.reset(),
        false,
    );

    bench_vec("Global", c, Global, |_| {});
    bench_vec("ring_alloc::RingAlloc", c, ring_alloc.clone(), |_| {});

    #[cfg(feature = "std")]
    bench_vec("ring_alloc::OneRingAlloc", c, OneRingAlloc, |_| {});

    bench_vec("bumpalo::Bump", c, Bump { bump: &mut bump }, |b| b.reset());
    bench_vec(
        "blink_alloc::BlinkAlloc",
        c,
        BlinkAlloc { blink: &mut blink },
        |b| b.reset(),
    );
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
