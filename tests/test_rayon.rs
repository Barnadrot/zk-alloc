//! Reproducer for the rayon/zk-alloc interaction bug documented in
//! leanMultisig commit f5e2299b. Pulls Tom's regression test verbatim and
//! adds a few stress variants to characterize how reliably the bug fires.
//!
//! Mechanism:
//!   1. rayon::join from a non-worker thread routes through the global
//!      `crossbeam_deque::Injector`, which is a linked list of fixed-size
//!      blocks (BLOCK_CAP = 63 slots).
//!   2. If a fresh injector block is allocated *during* an arena phase,
//!      the block lives in the arena slab.
//!   3. The next `begin_phase()` recycles the slab. Rayon still holds a
//!      pointer to that block; the next push writes a JobRef over whatever
//!      the application has allocated on top — silent corruption.
//!
//! These tests use #[global_allocator] so that rayon's allocations route
//! through ZkAllocator (otherwise they go to the system allocator and
//! can't be corrupted).

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

/// Tom's original MRE.
#[test]
fn rayon_does_not_corrupt_zkalloc() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    zk_alloc::begin_phase();
    for _ in 0..200 {
        rayon::join(|| {}, || {});
    }
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let canary = vec![0xAB_u8; 8192];
    rayon::join(|| {}, || {});
    zk_alloc::end_phase();

    let pos = canary.iter().position(|&b| b != 0xAB);
    assert!(pos.is_none(), "canary corrupted at offset {}", pos.unwrap());
}
