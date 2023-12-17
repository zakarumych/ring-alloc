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

## Usage

This crate provides two types of ring-allocators.
[`RingAlloc`] is a thread-local allocator that owns its rings of chunks
and uses user-provided underlying allocator to allocate chunks.
[`RingAlloc`] is cheaply clonnable and clones share internal state.

```rust
#![cfg_attr(feature = "nightly", feature(allocator_api))]

use ring_alloc::RingAlloc;
use allocator_api2::{boxed::Box, vec::Vec};

#[cfg(feature = "alloc")]
fn foo() -> Vec<Box<u32, RingAlloc>, RingAlloc> {
    let alloc = RingAlloc::new();
    let b = Box::new_in(42, alloc.clone());

    let mut v = Vec::new_in(alloc);
    v.push(b);
    v
}

fn main() {
    #[cfg(feature = "alloc")]
    {
        let v = foo();
        assert_eq!(*v[0], 42);
    }
}
```

[`OneRingAlloc`] is ZST allocator that uses global-state and thread-local
storage.
It can be used across threads and may transfer chunks between threads
when thread exists with chunks that are still in use.

[`OneRingAlloc`] always uses global allocator to allocate chunks.


```rust
#![cfg_attr(feature = "nightly", feature(allocator_api))]

#[cfg(feature = "std")]
use ring_alloc::OneRingAlloc;
use allocator_api2::{boxed::Box, vec::Vec};

#[cfg(feature = "std")]
fn foo() -> Vec<Box<u32, OneRingAlloc>, OneRingAlloc> {
    let b = Box::new_in(42, OneRingAlloc);

    let mut v = Vec::new_in(OneRingAlloc);
    v.push(b);
    v
}

fn main() {
    #[cfg(feature = "std")]
    {
        let v = std::thread::spawn(foo).join().unwrap();
        assert_eq!(*v[0], 42);
    }
}
```

Allocators are usable on stable Rust with [`allocator-api2`] crate.
"nightly" feature enables support for unstable Rust `allocator_api`,
available on nightly compiler.

[`RingAlloc`]: https://docs.rs/ring-alloc/0.1.0/ring_alloc/struct.RingAlloc.html
[`OneRingAlloc`]: https://docs.rs/ring-alloc/0.1.0/ring_alloc/struct.OneRingAlloc.html
[`allocator-api2`]: https://crates.io/crates/allocator-api2


## Benchmarks

### warm-up

|                             | `Global`                | `ring_alloc::RingAlloc`           | `ring_alloc::OneRingAlloc`          | `bumpalo::Bump`                  | `blink_alloc::BlinkAlloc`          |
|:----------------------------|:------------------------|:----------------------------------|:------------------------------------|:---------------------------------|:-----------------------------------|
| **`alloc 4 bytes x 65535`** | `2.79 ms` (✅ **1.00x**) | `209.38 us` (🚀 **13.31x faster**) | `308.28 us` (🚀 **9.04x faster**)    | `343.33 us` (🚀 **8.12x faster**) | `158.02 us` (🚀 **17.64x faster**)  |

### allocation

|             | `Global`                 | `ring_alloc::RingAlloc`          | `ring_alloc::OneRingAlloc`          | `bumpalo::Bump`                | `blink_alloc::BlinkAlloc`          |
|:------------|:-------------------------|:---------------------------------|:------------------------------------|:-------------------------------|:-----------------------------------|
| **`alloc`** | `23.99 ns` (✅ **1.00x**) | `5.15 ns` (🚀 **4.65x faster**)   | `11.18 ns` (🚀 **2.15x faster**)     | `7.39 ns` (🚀 **3.25x faster**) | `2.48 ns` (🚀 **9.69x faster**)     |

### vec

|                                | `Global`                  | `ring_alloc::RingAlloc`          | `ring_alloc::OneRingAlloc`          | `bumpalo::Bump`                  | `blink_alloc::BlinkAlloc`          |
|:-------------------------------|:--------------------------|:---------------------------------|:------------------------------------|:---------------------------------|:-----------------------------------|
| **`push x 10`**                | `97.75 ns` (✅ **1.00x**)  | `32.31 ns` (🚀 **3.03x faster**)  | `41.61 ns` (🚀 **2.35x faster**)     | `33.14 ns` (🚀 **2.95x faster**)  | `25.61 ns` (🚀 **3.82x faster**)    |
| **`reserve_exact(1) x 10`**    | `214.64 ns` (✅ **1.00x**) | `83.26 ns` (🚀 **2.58x faster**)  | `120.62 ns` (✅ **1.78x faster**)    | `72.62 ns` (🚀 **2.96x faster**)  | `57.14 ns` (🚀 **3.76x faster**)    |
| **`push x 146`**               | `490.41 ns` (✅ **1.00x**) | `373.99 ns` (✅ **1.31x faster**) | `376.81 ns` (✅ **1.30x faster**)    | `337.93 ns` (✅ **1.45x faster**) | `331.61 ns` (✅ **1.48x faster**)   |
| **`reserve_exact(1) x 146`**   | `4.08 us` (✅ **1.00x**)   | `2.03 us` (🚀 **2.01x faster**)   | `2.58 us` (✅ **1.58x faster**)      | `1.92 us` (🚀 **2.12x faster**)   | `1.46 us` (🚀 **2.79x faster**)     |
| **`push x 2134`**              | `5.05 us` (✅ **1.00x**)   | `5.23 us` (✅ **1.04x slower**)   | `5.33 us` (✅ **1.06x slower**)      | `5.06 us` (✅ **1.00x slower**)   | `4.96 us` (✅ **1.02x faster**)     |
| **`reserve_exact(1) x 2134`**  | `50.44 us` (✅ **1.00x**)  | `209.66 us` (❌ *4.16x slower*)   | `223.18 us` (❌ *4.42x slower*)      | `211.02 us` (❌ *4.18x slower*)   | `211.70 us` (❌ *4.20x slower*)     |
| **`push x 17453`**             | `43.56 us` (✅ **1.00x**)  | `41.80 us` (✅ **1.04x faster**)  | `42.02 us` (✅ **1.04x faster**)     | `41.66 us` (✅ **1.05x faster**)  | `42.59 us` (✅ **1.02x faster**)    |
| **`reserve_exact(1) x 17453`** | `432.89 us` (✅ **1.00x**) | `13.44 ms` (❌ *31.04x slower*)   | `13.05 ms` (❌ *30.14x slower*)      | `21.12 ms` (❌ *48.79x slower*)   | `19.60 ms` (❌ *45.27x slower*)     |

---
Made with [criterion-table](https://github.com/nu11ptr/criterion-table)

### Conclusion

`RingAlloc` is faster than `bumpalo` in most cases.
`OneRingAlloc` is slower than `RingAlloc` and `bumpalo` in exchange for multi-threading support.

`Global` allocator shows better results on `reserve_exact(1)` tests because it
provides optimized `Allocator::grow`, not yet implemented in `RingAlloc`.
`Global` allocator is slightly better on `push` for large vector.
`RingAlloc` directs large allocations to underlying allocator, which is `Global` in tests.

## License

Licensed under either of

* Apache License, Version 2.0, ([license/APACHE](license/APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([license/MIT](license/MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
