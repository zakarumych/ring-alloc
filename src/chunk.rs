use core::{
    alloc::Layout,
    cell::{Cell, UnsafeCell},
    mem::{align_of, size_of, MaybeUninit},
    ptr::NonNull,
    sync::atomic::Ordering,
};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::{addr, with_addr, with_addr_mut, ImUsize};

pub(crate) const BARE_ALLOCATION_CHUNK_SIZE_THRESHOLD: usize = 16384;

#[repr(C)]
pub(crate) struct ChunkHeader<T, const N: usize> {
    cursor: Cell<usize>,
    freed: T,
    pub next: Cell<Option<NonNull<Chunk<T, N>>>>,
}

#[repr(C)]
pub(crate) struct Chunk<T, const N: usize> {
    header: ChunkHeader<T, N>,
    memory: UnsafeCell<MaybeUninit<u8>>,
}

impl<T, const N: usize> Chunk<T, N>
where
    T: ImUsize,
{
    const SIZE: usize = N;

    const BARE_ALLOCATION_CHUNK: bool = Self::SIZE <= BARE_ALLOCATION_CHUNK_SIZE_THRESHOLD;

    const ALIGNMENT: usize = {
        if Self::BARE_ALLOCATION_CHUNK {
            BARE_ALLOCATION_CHUNK_SIZE_THRESHOLD
        } else {
            align_of::<usize>()
        }
    };

    const LAYOUT: Layout = match Layout::from_size_align(Self::SIZE, Self::ALIGNMENT) {
        Ok(layout) => layout,
        Err(_) => panic!("Invalid chunk size"),
    };

    const CHECK_SIZE_VALID: () = {
        if Self::SIZE < size_of::<Self>() {
            panic!("Chunk size is too small");
        }
    };

    pub fn new<'a, A>(alloc: A) -> Result<&'a mut Self, AllocError>
    where
        A: Allocator + 'a,
    {
        let () = Self::CHECK_SIZE_VALID;

        let ptr = alloc.allocate(Self::LAYOUT)?.cast::<Self>();

        // Safety: Ptr is valid but uninit.
        let header_ptr = unsafe { core::ptr::addr_of_mut!((*ptr.as_ptr()).header) };
        let cursor = addr(header_ptr);

        // Safety: Initializing header
        unsafe {
            core::ptr::write(
                header_ptr,
                ChunkHeader {
                    cursor: Cell::new(cursor),
                    freed: T::new(cursor), // Free cursor follows same scheme as allocation cursor.
                    next: Cell::new(None),
                },
            );
        }

        // Safety: All non-`MaybeUninit` fields are initialized.
        let chunk = unsafe { &mut *ptr.as_ptr() };
        Ok(chunk)
    }

    /// # Safety
    ///
    /// `ptr` must be valid pointer to `Self` allocated by `alloc` using same allocator
    /// or compatible one.
    pub unsafe fn free<A>(ptr: NonNull<Self>, alloc: A)
    where
        A: Allocator,
    {
        // Safety: `ptr` is valid pointer to `Self` allocated by `alloc`.
        unsafe {
            alloc.deallocate(ptr.cast(), Self::LAYOUT);
        }
    }

    fn base_addr(&self) -> usize {
        addr(self as *const Self)
    }

    fn end_addr(&self) -> usize {
        self.base_addr() + N
    }

    unsafe fn with_addr(&self, addr: usize) -> *mut u8 {
        unsafe { with_addr_mut(self.memory.get().cast(), addr) }
    }

    pub fn header(&self) -> &ChunkHeader<T, N> {
        // Safety: Header is initialized in `new`.
        unsafe { &*self.memory.get().cast() }
    }

    pub fn header_mut(&mut self) -> &mut ChunkHeader<T, N> {
        // Safety: Header is initialized in `new`.
        unsafe { &mut *self.memory.get().cast() }
    }

    /// Returns pointer to the next chunk in the ring.
    pub fn next(&self) -> Option<NonNull<Self>> {
        self.header().next.get()
    }

    /// Returns cursor position in the chunk.
    fn cursor(&self) -> &Cell<usize> {
        &self.header().cursor
    }

    /// Returns free "cursor" position in the chunk.
    fn freed(&self) -> &T {
        &self.header().freed
    }
}

impl<T, const N: usize> Chunk<T, N>
where
    T: ImUsize,
{
    /// Checks if chunk is unused.
    /// This state can be changed by calling `allocate`.
    ///
    /// If chunk is potentially shared, this method may return `true`
    /// while another thread is allocating from this chunk.
    #[inline(always)]
    pub fn unused(&self) -> bool {
        self.freed().load(Ordering::Acquire) == self.cursor().get()
    }

    #[inline(always)]
    fn _allocate(&self, layout: Layout) -> Option<NonNull<u8>> {
        let mut cursor = self.cursor().get();

        // Reuse chunk if it is freed.
        // Sync with `Release` in `deallocate`.
        if self.freed().load(Ordering::Acquire) == cursor {
            cursor = self.base_addr();
            self.freed().store(cursor, Ordering::Relaxed);
            self.cursor().set(cursor);
        }

        let aligned = cursor.checked_add(layout.align() - 1)? & !(layout.align() - 1);
        let new_cursor = aligned.checked_add(layout.size())?;
        if new_cursor > self.end_addr() {
            return None;
        }

        // Safety: `aligned` is within the chunk.
        let ptr = unsafe { self.with_addr(aligned) };
        self.cursor().set(new_cursor);

        // Safety: `freed` is always not greater than `cursor`.
        // So this cannot overflow.
        let overhead = aligned - cursor;
        self.freed().fetch_add(overhead, Ordering::Relaxed);

        // Safety: Range form `ptr` to `ptr + layout.size()` is within the chunk.
        Some(unsafe { NonNull::new_unchecked(ptr) })
    }

    #[inline(always)]
    fn allocate_bare(&self, layout: Layout) -> Option<NonNull<u8>> {
        self._allocate(layout)
    }

    #[inline(always)]
    fn allocate_meta(&self, layout: Layout) -> Option<NonNull<u8>> {
        let (meta_layout, offset) = Layout::new::<usize>().extend(layout).ok()?;
        let ptr = self._allocate(meta_layout)?;

        // Safety: `ptr` is allocated to contain `usize` followed with memory for `layout`.
        unsafe {
            ptr.as_ptr().cast::<usize>().write(self.base_addr());
        }

        // Safety: offset for `layout` in `meta_layout` used to calculate `ptr`.
        let ptr = unsafe { ptr.as_ptr().add(offset) };

        // Safety: `ptr` is allocation for `layout`.
        Some(unsafe { NonNull::new_unchecked(ptr) })
    }

    #[inline(always)]
    pub fn allocate(&self, layout: Layout) -> Option<NonNull<u8>> {
        if Self::BARE_ALLOCATION_CHUNK {
            self.allocate_bare(layout)
        } else {
            self.allocate_meta(layout)
        }
    }

    #[inline(always)]
    unsafe fn _deallocate(&self, size: usize) {
        // Safety: `freed` is always less than `cursor - size`.
        // Sync with `Acquire` in `allocate`.
        self.freed().fetch_add(size, Ordering::Release);
    }

    #[inline(always)]
    unsafe fn deallocate_bare(ptr: *const u8, size: usize) {
        let addr = addr(ptr);
        let chunk_addr = addr & !(Self::SIZE - 1);

        // Safety: `chunk_addr` is correct address for `Self`.
        let chunk = unsafe { with_addr(ptr, chunk_addr) }.cast::<Self>();

        // Safety: chunk is alive since `ptr` is alive.
        let chunk = unsafe { &*chunk };

        unsafe {
            chunk._deallocate(size);
        }
    }

    #[inline(always)]
    unsafe fn deallocate_meta(ptr: *const u8, layout: Layout) {
        let ptr_addr = addr(ptr);
        let meta_addr = (ptr_addr - size_of::<usize>()) & !(layout.align() - 1);

        let meta_ptr = unsafe { with_addr(ptr, meta_addr) }.cast::<usize>();
        let chunk_addr = unsafe { *meta_ptr };

        // Safety: `chunk_addr` is correct address for `Self`.
        // `ptr` is allocated from `chunk`.
        let chunk = unsafe { with_addr(ptr, chunk_addr) }.cast::<Self>();

        // Safety: chunk is alive since `ptr` is alive.
        let chunk = unsafe { &*chunk };
        unsafe {
            chunk._deallocate(layout.size());
        }
    }

    #[inline(always)]
    pub unsafe fn deallocate(ptr: *const u8, layout: Layout) {
        if Self::BARE_ALLOCATION_CHUNK {
            unsafe {
                Self::deallocate_bare(ptr, layout.size());
            }
        } else {
            unsafe {
                Self::deallocate_meta(ptr, layout);
            }
        }
    }
}
