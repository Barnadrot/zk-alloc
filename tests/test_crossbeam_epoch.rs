//! Scenario 1: empirical test for crossbeam-epoch deferred garbage.
//!
//! crossbeam-deque uses crossbeam-epoch to defer-deallocate retired Buffers.
//! Each thread keeps a Local with a list of Bag<Garbage> nodes (~1.5 KB
//! each). Bag nodes themselves are heap-allocated; if allocated during a
//! phase, they live in the arena slab. If the slab is recycled before
//! crossbeam-epoch processes the bag, walking the garbage list reads
//! recycled bytes → silent corruption or crash inside crossbeam.
//!
//! F6 (source audit) hypothesized this is covered by size-routing (Bags <
//! 4 KB go to System). Empirical test: drive many Buffer resizes during a
//! phase to retire many objects to crossbeam-epoch, cross a phase boundary,
//! drive more retires, and assert program integrity over many cycles.

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

/// Force per-worker crossbeam-deque buffer growth via deep recursion. Each
/// growth retires the prior buffer to crossbeam-epoch.
fn nested_join(depth: usize) {
    if depth == 0 {
        return;
    }
    rayon::join(|| nested_join(depth - 1), || {});
}

#[test]
fn crossbeam_epoch_garbage_survives_phase_cycles() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    const CYCLES: usize = 50;
    for _ in 0..CYCLES {
        // Phase 1: drive buffer growth → retire old buffers to epoch garbage.
        // Depth 1024 → buffer grows 32 → 64 → 128 → 256 → 512 → 1024 → 2048
        // (six resizes per worker that participates).
        zk_alloc::begin_phase();
        rayon::join(|| nested_join(1024), || {});
        zk_alloc::end_phase();

        // Phase 2: drive more growth + epoch participation. If a Bag from
        // phase 1 was allocated in arena and its slab was recycled, this
        // would crash inside crossbeam-epoch's collect().
        zk_alloc::begin_phase();
        rayon::join(|| nested_join(1024), || {});
        zk_alloc::end_phase();
    }

    eprintln!(
        "crossbeam_epoch_garbage_survives_phase_cycles: {CYCLES} cycles OK (MIN_ARENA_BYTES={})",
        zk_alloc::min_arena_bytes()
    );
}

/// par_iter with collect — drives crossbeam-channel + crossbeam-deque
/// allocations through normal rayon usage. Used to confirm typical
/// rayon-heavy workloads survive 100 cycles.
#[test]
fn crossbeam_in_par_iter_collect_survives_cycles() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    for _ in 0..100 {
        zk_alloc::begin_phase();
        let v: Vec<u64> = (0..4096_u64)
            .into_par_iter()
            .map(|i| {
                let mut acc = 0u64;
                for j in 0..32 {
                    acc = acc.wrapping_add((i * j) ^ 0xDEADBEEF);
                }
                acc
            })
            .collect();
        std::hint::black_box(v);
        zk_alloc::end_phase();
    }
}
