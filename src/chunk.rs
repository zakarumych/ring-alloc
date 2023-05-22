use core::{
    alloc::Layout,
    cell::Cell,
    mem::{align_of, size_of},
    ptr::NonNull,
    sync::atomic::Ordering,
};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::{addr, with_addr_mut, ImUsize};

#[repr(C)]
#[derive(Debug)]
pub(crate) struct Chunk<T, const N: usize> {
    pub cursor: Cell<*mut u8>,
    pub freed: T,
    pub next: Cell<Option<NonNull<Chunk<T, N>>>>,
}

impl<T, const N: usize> Chunk<T, N>
where
    T: ImUsize,
{
    const SIZE: usize = N;

    const ALIGNMENT: usize = align_of::<Self>();

    const LAYOUT: Layout = match Layout::from_size_align(Self::SIZE, Self::ALIGNMENT) {
        Ok(layout) => layout,
        Err(_) => panic!("Invalid chunk size"),
    };

    const LAYOUT_IS_VALID: bool = {
        if Self::SIZE < size_of::<Self>() {
            panic!("Chunk size is too small");
        }
        if Self::ALIGNMENT < align_of::<Self>() {
            panic!("Chunk alignment is too small");
        }
        true
    };

    pub fn new<'a, A>(alloc: A) -> Result<NonNull<Self>, AllocError>
    where
        A: Allocator + 'a,
    {
        debug_assert!(Self::LAYOUT_IS_VALID);

        let ptr = alloc.allocate(Self::LAYOUT)?.cast::<Self>();
        let memory = unsafe { ptr.as_ptr().add(1).cast::<u8>() };

        // Safety: Writing into memory allocated for `Chunk`.
        unsafe {
            ptr.as_ptr().write(Chunk {
                cursor: Cell::new(memory),
                freed: T::new(addr(memory)),
                next: Cell::new(None),
            });
        }

        Ok(ptr.cast())
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

    fn chunk_addr(&self) -> usize {
        addr(self as *const Self)
    }

    fn base_addr(&self) -> usize {
        self.chunk_addr() + size_of::<Self>()
    }

    fn end_addr(&self) -> usize {
        self.chunk_addr() + N
    }

    // unsafe fn with_addr(&self, addr: usize) -> *mut u8 {
    //     unsafe { with_addr_mut(self.memory.get().cast(), addr) }
    // }

    /// Returns pointer to the next chunk in the ring.
    pub fn next(&self) -> Option<NonNull<Self>> {
        self.next.get()
    }

    /// Returns cursor position in the chunk.
    fn cursor(&self) -> &Cell<*mut u8> {
        &self.cursor
    }

    /// Returns free "cursor" position in the chunk.
    fn freed(&self) -> &T {
        &self.freed
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
    #[inline(never)]
    pub fn unused(&self) -> bool {
        self.freed().load(Ordering::Acquire) == addr(self.cursor().get())
    }

    #[inline(never)]
    fn _allocate(&self, layout: Layout) -> Option<NonNull<u8>> {
        let mut cursor = self.cursor().get();

        // Reuse chunk if it is freed.
        // Sync with `Release` in `deallocate`.
        if self.freed().load(Ordering::Acquire) == addr(cursor) {
            // Safety: base_addr is beginning of the chunk memory
            // and cursor is within the chunk memory.
            cursor = unsafe { with_addr_mut(cursor, self.base_addr()) };
            self.freed().store(addr(cursor), Ordering::Relaxed);
            self.cursor().set(cursor);
        }

        let aligned = addr(cursor).checked_add(layout.align() - 1)? & !(layout.align() - 1);
        let new_cursor = aligned.checked_add(layout.size())?;
        if new_cursor > self.end_addr() {
            return None;
        }

        // Safety: `aligned` is within the chunk.
        let ptr = unsafe { with_addr_mut(cursor, aligned) };

        // Safety: `new_cursor` is within the chunk.
        let new_cursor = unsafe { with_addr_mut(cursor, new_cursor) };
        self.cursor().set(new_cursor);

        // Safety: `freed` is always not greater than `cursor`.
        // So this cannot overflow.
        let overhead = aligned - addr(cursor);
        self.freed().fetch_add(overhead, Ordering::Relaxed);

        // Safety: Range form `ptr` to `ptr + layout.size()` is within the chunk.
        Some(unsafe { NonNull::new_unchecked(ptr) })
    }

    #[inline(never)]
    pub fn allocate(&self, chunk_ptr: NonNull<Self>, layout: Layout) -> Option<NonNull<u8>> {
        let (meta_layout, offset) = Layout::new::<NonNull<Self>>().extend(layout).ok()?;
        let ptr = self._allocate(meta_layout)?;

        // Safety: `ptr` is allocated to contain `usize` followed with memory for `layout`.
        unsafe {
            ptr.as_ptr().cast::<NonNull<Self>>().write(chunk_ptr);
        }

        // Safety: offset for `layout` in `meta_layout` used to calculate `ptr`.
        let ptr = unsafe { ptr.as_ptr().add(offset) };

        // Safety: `ptr` is allocation for `layout`.
        Some(unsafe { NonNull::new_unchecked(ptr) })
    }

    #[inline(never)]
    unsafe fn _deallocate(&self, size: usize) {
        // Safety: `freed` is always less than `cursor - size`.
        // Sync with `Acquire` in `allocate`.
        self.freed().fetch_add(size, Ordering::Release);
    }

    #[inline(never)]
    pub unsafe fn deallocate(ptr: *mut u8, layout: Layout) {
        let (meta_layout, offset) = Layout::new::<NonNull<Self>>().extend(layout).unwrap();

        let meta_ptr = unsafe { ptr.sub(offset) }.cast::<NonNull<Self>>();
        let chunk_ptr = unsafe { *meta_ptr };

        // Safety: chunk is alive since `ptr` is alive.
        let chunk = unsafe { chunk_ptr.as_ref() };
        unsafe {
            chunk._deallocate(meta_layout.size());
        }
    }
}
