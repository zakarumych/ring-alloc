//!
//!
//! Ring-based allocator for Rust.
//! This allocator is more flexible compared to arena-based allocators since there's no
//! lifetime bounds.
//! However user should still deallocate memory in short time to avoid
//! wasting memory.
//!
//! Allocator uses ring buffer of chunks to allocate memory in front chunk,
//! moving it to back if chunk is full.
//! If next chunk is still occupied by previous allocation, allocator will
//! allocate new chunk.
//!
//! If all allocations are deallocated while ring makes one full turn,
//! no new chunks will be allocated.
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(allocator_api))]
#![warn(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod chunk;
mod local;

#[cfg(feature = "alloc")]
mod global;

use core::{alloc::Layout, cell::Cell, sync::atomic::Ordering};

pub use self::local::RingAlloc;

#[cfg(feature = "alloc")]
pub use self::global::OneRingAlloc;

fn addr<T: ?Sized>(ptr: *const T) -> usize {
    // Safety: pointer to address conversion is always valid.
    unsafe { core::mem::transmute(ptr.cast::<()>()) }
}

/// # Safety
///
/// New address must be withing same allocation as `ptr`.
/// Address must be aligned for `T`.
/// `addr` must be non-zero.
unsafe fn with_addr_mut<T>(ptr: *mut T, dest_addr: usize) -> *mut T {
    let ptr_addr = addr(ptr) as isize;
    let offset = (dest_addr as isize).wrapping_sub(ptr_addr);
    ptr.cast::<u8>().wrapping_offset(offset).cast()
}

trait ImUsize {
    fn new(value: usize) -> Self;
    fn load(&self, ordering: Ordering) -> usize;
    fn store(&self, value: usize, ordering: Ordering);
    fn fetch_add(&self, value: usize, ordering: Ordering) -> usize;
}

impl ImUsize for Cell<usize> {
    #[inline(never)]
    fn new(value: usize) -> Self {
        Cell::new(value)
    }

    #[inline(never)]
    fn load(&self, _ordering: Ordering) -> usize {
        self.get()
    }

    #[inline(never)]
    fn store(&self, value: usize, _ordering: Ordering) {
        self.set(value)
    }

    #[inline(never)]
    fn fetch_add(&self, value: usize, _ordering: Ordering) -> usize {
        let old_value = self.get();
        self.set(old_value.wrapping_add(value));
        old_value
    }
}

#[cfg(feature = "std")]
impl ImUsize for core::sync::atomic::AtomicUsize {
    #[inline(never)]
    fn new(value: usize) -> Self {
        Self::new(value)
    }

    #[inline(never)]
    fn load(&self, ordering: Ordering) -> usize {
        self.load(ordering)
    }

    #[inline(never)]
    fn store(&self, value: usize, ordering: Ordering) {
        self.store(value, ordering)
    }

    #[inline(never)]
    fn fetch_add(&self, value: usize, ordering: Ordering) -> usize {
        self.fetch_add(value, ordering)
    }
}

#[inline(never)]
fn layout_max(layout: Layout) -> usize {
    layout.align().max(layout.size())
}

#[cfg(test)]
mod tests;
