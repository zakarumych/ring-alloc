# ring-alloc

[![crates](https://img.shields.io/crates/v/ring-alloc.svg?style=for-the-badge&label=ring-alloc)](https://crates.io/crates/ring-alloc)
[![docs](https://img.shields.io/badge/docs.rs-ring--alloc-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white)](https://docs.rs/ring-alloc)
[![actions](https://img.shields.io/github/actions/workflow/status/zakarumych/ring-alloc/badge.yml?branch=main&style=for-the-badge)](https://github.com/zakarumych/ring-alloc/actions/workflows/badge.yml)
[![MIT/Apache](https://img.shields.io/badge/license-MIT%2FApache-blue.svg?style=for-the-badge)](COPYING)
![loc](https://img.shields.io/tokei/lines/github/zakarumych/ring-alloc?style=for-the-badge)


Ring-based memory allocator for Rust to use for short-living allocations.

Provides better flexibility compared to arena-based allocators
since there's no lifetime bounds.
However user should still deallocate memory in short time to avoid
wasting memory.

Allocator uses ring buffer of chunks to allocate memory in front chunk,
moving it to back when full.
If next chunk is still occupied by old allocations, allocator will
allocate new chunk.
When there's enough chunks so that next chunk is always unoccupied,
when first chunk becomes exhausted, it won't allocate new chunks anymore.

Failing to deallocate single block of memory will stop ring-allocator
from reusing chunks, so be careful to not leak blocks or keep them
alive for too long.

# Usage

This crate provides two types of ring-allocators.
[`RingAlloc`] is a thread-local allocator that owns its rings of chunks
and uses user-provided underlying allocator to allocate chunks.
[`RingAlloc`] is cheaply clonnable and clones share internal state.

```rust
#![cfg_attr(feature = "nightly", feature(allocator_api))]
use ring_alloc::RingAlloc;
use allocator_api2::{boxed::Box, vec::Vec};

fn foo() -> Vec<Box<u32, RingAlloc>, RingAlloc> {
    let alloc = RingAlloc::new();
    let b = Box::new_in(42, alloc.clone());

    let mut v = Vec::new_in(alloc);
    v.push(b);
    v
}

fn main() {
    let v = foo();
    assert_eq!(*v[0], 42);
}
```

[`OneRingAlloc`] is ZST allocator that uses global-state and thread-local
storage.
It can be used across threads and may transfer chunks between threads
when thread exists with chunks that are still in use.

[`OneRingAlloc`] always uses global allocator to allocate chunks.


```rust
#![cfg_attr(feature = "nightly", feature(allocator_api))]
use ring_alloc::OneRingAlloc;
use allocator_api2::{boxed::Box, vec::Vec};

fn foo() -> Vec<Box<u32, OneRingAlloc>, OneRingAlloc> {
    let b = Box::new_in(42, OneRingAlloc);

    let mut v = Vec::new_in(OneRingAlloc);
    v.push(b);
    v
}

fn main() {
    let v = std::thread::spawn(foo).join().unwrap();
    assert_eq!(*v[0], 42);
}
```

Allocators are usable on stable Rust with [`allocator-api2`] crate.
"nightly" feature enables support for unstable Rust `allocator_api`,
available on nightly compiler.

[`RingAlloc`]: https://docs.rs/ring-alloc/0.1.0/ring_alloc/struct.RingAlloc.html
[`OneRingAlloc`]: https://docs.rs/ring-alloc/0.1.0/ring_alloc/struct.OneRingAlloc.html
[`allocator-api2`]: https://crates.io/crates/allocator-api2

## License

Licensed under either of

* Apache License, Version 2.0, ([license/APACHE](license/APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([license/MIT](license/MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
