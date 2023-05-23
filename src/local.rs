use core::{
    cell::Cell,
    hash::{Hash, Hasher},
    mem::ManuallyDrop,
    ptr::NonNull,
};

use allocator_api2::alloc::{AllocError, Allocator, Layout};

use crate::layout_max;

type Chunk<const N: usize> = crate::chunk::Chunk<Cell<usize>, { N }>;

/// Allocations up to this number of bytes are allocated in the tiny chunk.
const TINY_ALLOCATION_MAX_SIZE: usize = 16;

/// Size of the chunk for allocations not larger than `TINY_ALLOCATION_CHUNK_SIZE`.
const TINY_ALLOCATION_CHUNK_SIZE: usize = 16384;

/// Allocations up to this number of bytes are allocated in the small chunk.
const SMALL_ALLOCATION_MAX_SIZE: usize = 256;

/// Size of the chunk for allocations not larger than `SMALL_ALLOCATION_MAX_SIZE`.
const SMALL_ALLOCATION_CHUNK_SIZE: usize = 65536;

/// Allocations up to this number of bytes are allocated in the large chunk.
const LARGE_ALLOCATION_MAX_SIZE: usize = 65536;

/// Size of the chunk for allocations larger than `SMALL_ALLOCATION_MAX_SIZE`.
const LARGE_ALLOCATION_CHUNK_SIZE: usize = 2097152;

#[cfg(not(feature = "alloc"))]
macro_rules! ring_alloc {
    ($(#[$meta:meta])* pub struct $ring_alloc:ident;) => {
        $(#[$meta])*
        #[repr(transparent)]
        pub struct $ring_alloc<A: Allocator> {
            inner: NonNull<Rings<A>>,
        }
    };
}

#[cfg(feature = "alloc")]
macro_rules! ring_alloc {
    ($(#[$meta:meta])* pub struct $ring_alloc:ident;) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[must_use]
        pub struct $ring_alloc<A: Allocator = allocator_api2::alloc::Global> {
            inner: NonNull<Rings<A>>,
        }
    };
}

ring_alloc! {
    /// Thread-local ring-allocator.
    ///
    /// This allocator uses underlying allocator to allocate memory chunks.
    ///
    /// Allocator uses ring buffer of chunks to allocate memory in front chunk,
    /// moving it to back if chunk is full.
    /// If next chunk is still occupied by previous allocation, allocator will
    /// allocate new chunk.
    pub struct RingAlloc;
}

impl<A> Clone for RingAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn clone(&self) -> Self {
        Rings::inc_ref(self.inner);
        RingAlloc { inner: self.inner }
    }

    #[inline(always)]
    fn clone_from(&mut self, source: &Self) {
        Rings::inc_ref(source.inner);
        self.inner = source.inner;
    }
}

impl<A> PartialEq for RingAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<A> Hash for RingAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl<A> Drop for RingAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn drop(&mut self) {
        Rings::dec_ref(self.inner);
    }
}

type TinyChunk = Chunk<{ TINY_ALLOCATION_CHUNK_SIZE }>;
type SmallChunk = Chunk<{ SMALL_ALLOCATION_CHUNK_SIZE }>;
type LargeChunk = Chunk<{ LARGE_ALLOCATION_CHUNK_SIZE }>;

struct Ring<T> {
    // Head of the ring.
    // This is the current chunk.
    // When chunk is full, this chunk is moved to the end.
    head: Cell<Option<NonNull<T>>>,

    // Tail of the ring.
    tail: Cell<Option<NonNull<T>>>,
}

impl<T> Ring<T> {
    const fn new() -> Self {
        Ring {
            head: Cell::new(None),
            tail: Cell::new(None),
        }
    }
}

struct Rings<A: Allocator> {
    tiny_ring: Ring<TinyChunk>,
    small_ring: Ring<SmallChunk>,
    large_ring: Ring<LargeChunk>,
    allocator: ManuallyDrop<A>,
    ref_cnt: Cell<usize>,
}

impl<A> Rings<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn try_new_in(allocator: A) -> Result<NonNull<Self>, AllocError> {
        let ptr = allocator.allocate(Layout::new::<Self>())?;
        let inner = Rings {
            tiny_ring: Ring::new(),
            small_ring: Ring::new(),
            large_ring: Ring::new(),
            allocator: ManuallyDrop::new(allocator),
            ref_cnt: Cell::new(1),
        };

        let ptr = ptr.cast::<Self>();

        // Safety: `ptr` is valid pointer to `Self` allocated by `allocator`.
        unsafe {
            core::ptr::write(ptr.as_ptr(), inner);
        }

        Ok(ptr)
    }

    #[inline(always)]
    #[cfg(not(no_global_oom_handling))]
    fn new_in(allocator: A) -> NonNull<Self> {
        match Self::try_new_in(allocator) {
            Ok(ptr) => ptr,
            #[cfg(feature = "alloc")]
            Err(AllocError) => {
                alloc::alloc::handle_alloc_error(Layout::new::<Self>());
            }
            #[cfg(not(feature = "alloc"))]
            Err(AllocError) => {
                core::panic!("Failed to allocate Rings");
            }
        }
    }

    fn inc_ref(ptr: NonNull<Self>) {
        // Safety: `ptr` is valid pointer to `Self`.
        let me = unsafe { ptr.as_ref() };
        me.ref_cnt.set(me.ref_cnt.get() + 1);
    }

    fn dec_ref(ptr: NonNull<Self>) {
        // Safety: `ptr` is valid pointer to `Self`.
        let me = unsafe { ptr.as_ref() };

        debug_assert_ne!(me.ref_cnt.get(), 0);
        let new_ref_cnt = me.ref_cnt.get() - 1;
        me.ref_cnt.set(new_ref_cnt);

        if new_ref_cnt == 0 {
            Self::free(ptr);
        }
    }

    #[cold]
    fn free(ptr: NonNull<Self>) {
        // Safety: `ptr` is valid pointer to `Self`.
        let me = unsafe { ptr.as_ref() };

        me.free_all();

        // Safety: taking allocator out `ManuallyDrop`.
        // The value is dropped immediately after.
        let allocator = unsafe { core::ptr::read(&*me.allocator) };

        // Safety: `ptr` was allocated by `me.allocator`.
        unsafe {
            allocator.deallocate(ptr.cast(), Layout::new::<Self>());
        }
    }

    #[inline(always)]
    fn clean_all(&self) {
        Self::clean(&self.tiny_ring, &self.allocator);
        Self::clean(&self.small_ring, &self.allocator);
        Self::clean(&self.large_ring, &self.allocator);
    }

    #[inline(always)]
    fn clean<const N: usize>(ring: &Ring<Chunk<N>>, allocator: &A) {
        let mut chunk = &ring.head;

        while let Some(c) = chunk.get() {
            if unsafe { c.as_ref().unused() } {
                // Safety: chunks in the ring are always valid.
                chunk.set(unsafe { c.as_ref().next() });

                // Safety: `c` is valid pointer to `Chunk` allocated by `allocator`.
                unsafe {
                    Chunk::free(c, allocator);
                }
            } else {
                // Safety: chunks in the ring are always valid.
                chunk = unsafe { &c.as_ref().next };
            }
        }

        if ring.head.get().is_none() {
            ring.tail.set(None);
        }
    }

    fn free_all(&self) {
        Self::free_chunks(&self.tiny_ring, &self.allocator);
        Self::free_chunks(&self.small_ring, &self.allocator);
        Self::free_chunks(&self.large_ring, &self.allocator);
    }

    #[inline(always)]
    fn free_chunks<const N: usize>(ring: &Ring<Chunk<N>>, allocator: &A) {
        let mut chunk = ring.head.take();

        while let Some(c) = chunk {
            // Safety: chunks in the ring are always valid.
            chunk = unsafe { c.as_ref().next() };
            // Safety: `c` is valid pointer to `Chunk` allocated by `allocator`.
            unsafe {
                Chunk::free(c, allocator);
            }
        }

        ring.tail.set(None);
    }
}

#[cfg(not(no_global_oom_handling))]
#[cfg(feature = "alloc")]
impl RingAlloc {
    /// Returns new [`RingAlloc`] that uses [`Global`] allocator.
    #[inline(always)]
    pub fn new() -> Self {
        RingAlloc {
            inner: Rings::new_in(allocator_api2::alloc::Global),
        }
    }
}

#[cfg(not(no_global_oom_handling))]
impl<A> Default for RingAlloc<A>
where
    A: Allocator + Default,
{
    #[inline(always)]
    fn default() -> Self {
        RingAlloc::new_in(A::default())
    }
}

impl<A> RingAlloc<A>
where
    A: Allocator,
{
    /// Returns new [`RingAlloc`] that uses given allocator.
    #[cfg(not(no_global_oom_handling))]
    #[inline(always)]
    pub fn new_in(allocator: A) -> Self {
        RingAlloc {
            inner: Rings::new_in(allocator),
        }
    }

    /// Attempts to create new [`RingAlloc`] that uses given allocator.
    #[inline(always)]
    pub fn try_new_in(allocator: A) -> Result<Self, AllocError> {
        Ok(RingAlloc {
            inner: Rings::try_new_in(allocator)?,
        })
    }

    /// Attempts to allocate a block of memory with this ring-allocator.
    /// Returns a pointer to the beginning of the block if successful.
    #[inline(always)]
    pub fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Safety: `self.inner` is valid pointer to `Rings`
        let inner = unsafe { self.inner.as_ref() };
        if layout_max(layout) <= TINY_ALLOCATION_MAX_SIZE {
            Self::_allocate(&inner.tiny_ring, layout, &inner.allocator)
        } else if layout_max(layout) <= SMALL_ALLOCATION_MAX_SIZE {
            Self::_allocate(&inner.small_ring, layout, &inner.allocator)
        } else if layout_max(layout) <= LARGE_ALLOCATION_MAX_SIZE {
            Self::_allocate(&inner.large_ring, layout, &inner.allocator)
        } else {
            inner.allocator.allocate(layout)
        }
    }

    /// Deallocates the memory referenced by `ptr`.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a block of memory [*currently allocated*] via [`RingAlloc::allocate`], and
    /// * `layout` must [*fit*] that block of memory.
    ///
    /// [*currently allocated*]: https://doc.rust-lang.org/std/alloc/trait.Allocator.html#currently-allocated-memory
    /// [*fit*]: https://doc.rust-lang.org/std/alloc/trait.Allocator.html#memory-fitting
    #[inline(always)]
    pub unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout_max(layout) <= TINY_ALLOCATION_MAX_SIZE {
            unsafe {
                Self::_deallocate::<{ TINY_ALLOCATION_CHUNK_SIZE }>(ptr, layout);
            }
        } else if layout_max(layout) <= SMALL_ALLOCATION_MAX_SIZE {
            unsafe {
                Self::_deallocate::<{ SMALL_ALLOCATION_CHUNK_SIZE }>(ptr, layout);
            }
        } else if layout_max(layout) <= LARGE_ALLOCATION_MAX_SIZE {
            unsafe {
                Self::_deallocate::<{ LARGE_ALLOCATION_CHUNK_SIZE }>(ptr, layout);
            }
        } else {
            // Safety: `self.inner` is valid pointer to `Rings`
            let inner = unsafe { self.inner.as_ref() };
            // Safety: `ptr` is valid pointer allocated by `self.allocator`.
            unsafe {
                inner.allocator.deallocate(ptr, layout);
            }
        }
    }

    #[inline(always)]
    fn _allocate<const N: usize>(
        ring: &Ring<Chunk<N>>,
        layout: Layout,
        allocator: &A,
    ) -> Result<NonNull<[u8]>, AllocError> {
        // Try head chunk.
        if let Some(chunk_ptr) = ring.head.get() {
            // Safety: `chunk` is valid pointer to `Chunk` allocated by `self.allocator`.
            let chunk = unsafe { chunk_ptr.as_ref() };

            match chunk.allocate(chunk_ptr, layout) {
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
                None => match chunk.next.take() {
                    None => {
                        debug_assert_eq!(ring.tail.get(), ring.head.get());
                    }
                    Some(next_ptr) => {
                        // Move head to tail and bring next one as head.

                        // Safety: tail is valid pointer to `Chunk` allocated by `self.allocator`.
                        let tail_chunk = unsafe { ring.tail.get().unwrap().as_ref() };
                        debug_assert_eq!(tail_chunk.next(), None);
                        tail_chunk.next.set(Some(chunk_ptr));
                        ring.tail.set(Some(chunk_ptr));
                        ring.head.set(Some(next_ptr));

                        let next = unsafe { next_ptr.as_ref() };

                        if next.reset() {
                            if let Some(ptr) = next.allocate(next_ptr, layout) {
                                // Safety: `ptr` is valid pointer to `Chunk` allocated by `self.allocator`.
                                // ptr is allocated to fit `layout.size()` bytes.
                                return Ok(unsafe {
                                    NonNull::new_unchecked(core::ptr::slice_from_raw_parts_mut(
                                        ptr.as_ptr(),
                                        layout.size(),
                                    ))
                                });
                            }
                        }

                        // Not ready yet. Allocate new chunk.
                    }
                },
            }
        } else {
            debug_assert_eq!(ring.tail.get(), None);
        }

        let chunk_ptr = Chunk::<N>::new(allocator)?;

        // Safety: `chunk` is valid pointer to `Chunk` allocated by `self.allocator`.
        let chunk = unsafe { chunk_ptr.as_ref() };

        let ptr = chunk
            .allocate(chunk_ptr, layout)
            .expect("Failed to allocate from fresh chunk");

        // Put to head.
        chunk.next.set(ring.head.get());

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
    unsafe fn _deallocate<const N: usize>(ptr: NonNull<u8>, layout: Layout) {
        // Safety: `ptr` is valid pointer allocated from alive `Chunk`.
        unsafe {
            Chunk::<N>::deallocate(ptr.as_ptr(), layout);
        }
    }

    /// Free all unused chunks back to underlying allocator.
    pub fn flush(&self) {
        // Safety: `self.inner` is valid pointer to `Rings`
        let inner = unsafe { self.inner.as_ref() };
        inner.clean_all();
    }
}

unsafe impl<A> Allocator for RingAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.allocate(layout)
    }

    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Safety: covered by `Allocator::deallocate` contract.
        unsafe { self.deallocate(ptr, layout) }
    }

    // TODO: Implement grow and shrink.
}
