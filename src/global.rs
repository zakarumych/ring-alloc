use core::{
    alloc::Layout, cell::Cell, hint::unreachable_unchecked, ptr::NonNull, sync::atomic::AtomicUsize,
};
use std::thread_local;

use allocator_api2::alloc::{AllocError, Allocator, Global};
use parking_lot::Mutex;

use crate::{chunk::BARE_ALLOCATION_CHUNK_SIZE_THRESHOLD, layout_max};

type Chunk<const N: usize> = crate::chunk::Chunk<AtomicUsize, N>;

/// Allocations up to this number of bytes are allocated in the tiny chunk.
const TINY_ALLOCATION_MAX_SIZE: usize = 16;

/// Size of the chunk for allocations not larger than `TINY_ALLOCATION_CHUNK_SIZE`.
const TINY_ALLOCATION_CHUNK_SIZE: usize = BARE_ALLOCATION_CHUNK_SIZE_THRESHOLD;

/// Allocations up to this number of bytes are allocated in the small chunk.
const SMALL_ALLOCATION_MAX_SIZE: usize = 32;

/// Size of the chunk for allocations not larger than `SMALL_ALLOCATION_MAX_SIZE`.
const SMALL_ALLOCATION_CHUNK_SIZE: usize = 65536;

/// Allocations up to this number of bytes are allocated in the large chunk.
const LARGE_ALLOCATION_MAX_SIZE: usize = 65536;

/// Size of the chunk for allocations larger than `SMALL_ALLOCATION_MAX_SIZE`.
const LARGE_ALLOCATION_CHUNK_SIZE: usize = 2097152;

type TinyChunk = Chunk<{ TINY_ALLOCATION_CHUNK_SIZE }>;
type SmallChunk = Chunk<{ SMALL_ALLOCATION_CHUNK_SIZE }>;
type LargeChunk = Chunk<{ LARGE_ALLOCATION_CHUNK_SIZE }>;

struct LocalRing<T> {
    // Head of the ring.
    // This is the current chunk.
    // When chunk is full, this chunk is moved to the end.
    head: Cell<Option<NonNull<T>>>,

    // Tail of the ring.
    tail: Cell<Option<NonNull<T>>>,
}

impl<T> LocalRing<T> {
    const fn new() -> Self {
        LocalRing {
            head: Cell::new(None),
            tail: Cell::new(None),
        }
    }
}

struct GlobalRing<T> {
    // Head of the ring.
    // This is the current chunk.
    // When chunk is full, this chunk is moved to the end.
    head: Option<NonNull<T>>,

    // Tail of the ring.
    tail: Option<NonNull<T>>,
}

impl<T> GlobalRing<T> {
    const fn new() -> Self {
        GlobalRing {
            head: None,
            tail: None,
        }
    }
}

struct GlobalRings {
    tiny_ring: Mutex<GlobalRing<TinyChunk>>,
    small_ring: Mutex<GlobalRing<SmallChunk>>,
    large_ring: Mutex<GlobalRing<LargeChunk>>,
}

unsafe impl Send for GlobalRings {}
unsafe impl Sync for GlobalRings {}

struct LocalRings {
    tiny_ring: LocalRing<TinyChunk>,
    small_ring: LocalRing<SmallChunk>,
    large_ring: LocalRing<LargeChunk>,
}

impl Drop for LocalRings {
    fn drop(&mut self) {
        self.clean_all();
        self.flush_all();
    }
}

impl LocalRings {
    #[inline(always)]
    fn clean_all(&self) {
        Self::clean(&self.tiny_ring);
        Self::clean(&self.small_ring);
        Self::clean(&self.large_ring);
    }

    #[inline(always)]
    fn clean<const N: usize>(ring: &LocalRing<Chunk<N>>) {
        let mut chunk = &ring.head;

        while let Some(c) = chunk.get() {
            if unsafe { c.as_ref().unused() } {
                // Safety: chunks in the ring are always valid.
                chunk.set(unsafe { c.as_ref().next() });

                // Safety: `c` is valid pointer to `Chunk` allocated by `allocator`.
                unsafe {
                    Chunk::free(c, Global);
                }
            } else {
                // Safety: chunks in the ring are always valid.
                chunk = unsafe { &c.as_ref().header().next };
            }
        }

        ring.tail.set(None);
    }

    #[inline(always)]
    fn flush_all(&mut self) {
        Self::flush(&mut self.tiny_ring, &GLOBAL_RINGS.tiny_ring);
        Self::flush(&mut self.small_ring, &GLOBAL_RINGS.small_ring);
        Self::flush(&mut self.large_ring, &GLOBAL_RINGS.large_ring);
    }

    #[inline(always)]
    fn flush<const N: usize>(ring: &mut LocalRing<Chunk<N>>, global: &Mutex<GlobalRing<Chunk<N>>>) {
        match (ring.head.get(), ring.tail.get()) {
            (None, None) => {}
            (Some(head), Some(tail)) => {
                let mut global = global.lock();

                match (global.head, global.tail) {
                    (None, None) => {
                        global.tail = ring.tail.get();
                        global.head = ring.head.get();
                    }
                    (Some(_g_head), Some(mut g_tail)) => unsafe {
                        *g_tail.as_mut().header_mut().next.get_mut() = Some(head);
                        global.tail = Some(tail);
                    },
                    _ => unsafe { unreachable_unchecked() },
                }
            }
            _ => unsafe { unreachable_unchecked() },
        }
    }
}

thread_local! {
    static LOCAL_RINGS: LocalRings = const { LocalRings {
        tiny_ring: LocalRing::new(),
        small_ring: LocalRing::new(),
        large_ring: LocalRing::new(),
    } };
}

static GLOBAL_RINGS: GlobalRings = GlobalRings {
    tiny_ring: Mutex::new(GlobalRing::new()),
    small_ring: Mutex::new(GlobalRing::new()),
    large_ring: Mutex::new(GlobalRing::new()),
};

/// Global ring-allocator.
///
/// This allocator uses global allocator to allocate memory chunks.
///
/// Allocator uses ring buffer of chunks to allocate memory in front chunk,
/// moving it to back if chunk is full.
/// If next chunk is still occupied by previous allocation, allocator will
/// allocate new chunk.
///
/// This type is ZST and data is stored in static variables,
/// removing size overhead in collections.
///
/// Each thread will use thread-local rings to rotate over chunks.
/// On thread exit all unused chunks are freed and the rest is moved to global ring.
///
/// When thread-local ring cannot allocate memory it will steal global ring
/// or allocate new chunk from global allocator if global ring is empty.
pub struct OneRingAlloc;

#[inline]
fn _allocate<const N: usize>(
    ring: &LocalRing<Chunk<N>>,
    global: &Mutex<GlobalRing<Chunk<N>>>,
    layout: Layout,
) -> Result<NonNull<[u8]>, AllocError> {
    // Try head chunk.
    if let Some(chunk_ptr) = ring.head.get() {
        // Safety: `chunk` is valid pointer to `Chunk` allocated by `self.allocator`.
        let chunk = unsafe { chunk_ptr.as_ref() };

        match chunk.allocate(layout) {
            Some(ptr) => {
                // Safety: `ptr` is valid pointer to `Chunk` allocated by `self.allocator`.
                // ptr is allocated to fit `layout.size()` bytes.
                return Ok(unsafe {
                    NonNull::new_unchecked(core::ptr::slice_from_raw_parts_mut(
                        ptr.as_ptr(),
                        layout.size(),
                    ))
                });
            }
            // Chunk is full. Try next one.
            None => match chunk.header().next.take() {
                None => {
                    debug_assert_eq!(ring.tail.get(), ring.head.get());
                }
                Some(next_ptr) => {
                    // Move head to tail and bring next one as head.

                    // Safety: tail is valid pointer to `Chunk` allocated by `self.allocator`.
                    let tail_chunk = unsafe { ring.tail.get().unwrap().as_ref() };
                    debug_assert_eq!(tail_chunk.next(), None);
                    tail_chunk.header().next.set(Some(chunk_ptr));
                    ring.tail.set(Some(chunk_ptr));
                    ring.head.set(Some(next_ptr));

                    let next = unsafe { next_ptr.as_ref() };

                    if let Some(ptr) = next.allocate(layout) {
                        // Safety: `ptr` is valid pointer to `Chunk` allocated by `self.allocator`.
                        // ptr is allocated to fit `layout.size()` bytes.
                        return Ok(unsafe {
                            NonNull::new_unchecked(core::ptr::slice_from_raw_parts_mut(
                                ptr.as_ptr(),
                                layout.size(),
                            ))
                        });
                    }

                    // Not ready yet. Allocate new chunk.
                }
            },
        }
    } else {
        debug_assert_eq!(ring.tail.get(), None);
    }

    let (g_head, g_tail) = {
        let mut global = global.lock();
        // Take all chunks from global ring.
        (global.head.take(), global.tail.take())
    };

    let ptr = match (g_head, g_tail) {
        (None, None) => None,
        (Some(mut g_head), Some(mut g_tail)) => {
            let ptr = unsafe { g_head.as_mut().allocate(layout) };

            match (ring.head.get(), ring.tail.get()) {
                (None, None) => {
                    ring.head.set(Some(g_head));
                    ring.tail.set(Some(g_tail));
                }
                (Some(head), Some(_tail)) => unsafe {
                    *g_tail.as_mut().header_mut().next.get_mut() = Some(head);
                    ring.head.set(Some(g_head));
                },
                _ => unsafe { unreachable_unchecked() },
            }

            ptr
        }
        _ => unsafe { unreachable_unchecked() },
    };

    let ptr = match ptr {
        None => {
            let chunk = Chunk::<N>::new(Global)?;
            let ptr = chunk
                .allocate(layout)
                .expect("Failed to allocate from fresh chunk");

            // Put to head.
            chunk.header().next.set(ring.head.get());
            let chunk_ptr = NonNull::from(chunk);

            // If first chunk, put to tail.
            if ring.tail.get().is_none() {
                debug_assert_eq!(ring.head.get(), None);

                // Modify after asserts.
                ring.tail.set(Some(chunk_ptr));
            } else {
                debug_assert!(ring.head.get().is_some());
            }

            // Modify after asserts.
            ring.head.set(Some(chunk_ptr));

            ptr
        }
        Some(ptr) => ptr,
    };

    // Safety: `ptr` is valid pointer to `Chunk` allocated by `self.allocator`.
    // ptr is allocated to fit `layout.size()` bytes.
    Ok(unsafe {
        NonNull::new_unchecked(core::ptr::slice_from_raw_parts_mut(
            ptr.as_ptr(),
            layout.size(),
        ))
    })
}

#[inline(always)]
fn allocate(layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
    if layout_max(layout) <= TINY_ALLOCATION_MAX_SIZE {
        LOCAL_RINGS.with(|rings| _allocate(&rings.tiny_ring, &GLOBAL_RINGS.tiny_ring, layout))
    } else if layout_max(layout) <= SMALL_ALLOCATION_MAX_SIZE {
        LOCAL_RINGS.with(|rings| _allocate(&rings.small_ring, &GLOBAL_RINGS.small_ring, layout))
    } else if layout_max(layout) <= LARGE_ALLOCATION_MAX_SIZE {
        LOCAL_RINGS.with(|rings| _allocate(&rings.large_ring, &GLOBAL_RINGS.large_ring, layout))
    } else {
        Global.allocate(layout)
    }
}

#[inline(always)]
pub unsafe fn deallocate(ptr: NonNull<u8>, layout: Layout) {
    if layout_max(layout) <= TINY_ALLOCATION_MAX_SIZE {
        unsafe {
            _deallocate::<{ TINY_ALLOCATION_CHUNK_SIZE }>(ptr, layout);
        }
    } else if layout_max(layout) <= SMALL_ALLOCATION_MAX_SIZE {
        unsafe {
            _deallocate::<{ SMALL_ALLOCATION_CHUNK_SIZE }>(ptr, layout);
        }
    } else if layout_max(layout) <= LARGE_ALLOCATION_MAX_SIZE {
        unsafe {
            _deallocate::<{ LARGE_ALLOCATION_CHUNK_SIZE }>(ptr, layout);
        }
    } else {
        unsafe { Global.deallocate(ptr, layout) }
    }
}

#[inline(always)]
unsafe fn _deallocate<const N: usize>(ptr: NonNull<u8>, layout: Layout) {
    // Safety: `ptr` is valid pointer allocated from alive `Chunk`.
    unsafe {
        Chunk::<N>::deallocate(ptr.as_ptr(), layout);
    }
}

unsafe impl Allocator for OneRingAlloc {
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        allocate(layout)
    }

    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe {
            deallocate(ptr, layout);
        }
    }
}
