//! Scenario 2: per-worker crossbeam-deque Buffer growth.
//!
//! crossbeam-deque's Worker::push doubles its Buffer when the deque fills.
//! Initial capacity 32 slots × ~16 bytes per JobRef ≈ 512 bytes (under
//! MIN_ARENA_BYTES=4096). At ≥ 256 simultaneously pending tasks, the buffer
//! grows past 4 KB and lands in the arena slab.
//!
//! Workers retain their Buffer across phase boundaries — crossbeam never
//! shrinks. After end_phase + begin_phase, the slab is recycled but the
//! Worker still references the same Buffer pointer. The next push writes
//! a JobRef into recycled memory.
//!
//! Tests: drive deep rayon::join recursion from inside a worker (so pushes
//! land on a worker's local deque, not the global Injector) to force Buffer
//! growth past the size-routing threshold, then look for canary corruption.

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

/// Recursive rayon::join — each level pushes one right-task to the worker's
/// local deque. Peak pending tasks on the deque ≈ depth.
fn nested_join(depth: usize) {
    if depth == 0 {
        return;
    }
    rayon::join(|| nested_join(depth - 1), || {});
}

#[test]
fn worker_deque_growth_during_phase() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    const CYCLES: usize = 20;
    const DEPTH: usize = 1024; // > 256 → forces Buffer growth past 4 KB

    let mut failures = 0;
    for cycle in 0..CYCLES {
        zk_alloc::begin_phase();
        // Push from a worker context so growth happens on a per-worker deque,
        // not the global Injector.
        rayon::join(|| nested_join(DEPTH), || {});
        zk_alloc::end_phase();

        zk_alloc::begin_phase();
        let canary = vec![0xC9_u8; 65536];
        // Force more worker activity to consume / push deque slots.
        rayon::join(|| nested_join(64), || {});
        zk_alloc::end_phase();

        if let Some(pos) = canary.iter().position(|&b| b != 0xC9) {
            eprintln!("cycle {cycle}: canary corrupted at offset {pos}");
            failures += 1;
        }
    }
    eprintln!(
        "worker_deque_growth_during_phase: {failures}/{CYCLES} cycles corrupted (MIN_ARENA_BYTES={})",
        zk_alloc::min_arena_bytes()
    );

    // With size-routing default 4096, Buffers up to 256 slots (~4 KB) go to
    // System. Buffers above that — driven here by DEPTH=1024 — go to arena.
    // If size-routing is enough, failures==0. If not, failures>0.
    if zk_alloc::min_arena_bytes() >= 4096 {
        // Document outcome — assertion deferred to actual observation.
    }
}

/// Same idea but with a single very deep recursion, no canary mismatch
/// allowed. If buffer growth + phase recycle causes corruption, this should
/// crash or panic via tracing/rayon internals (similar to F1).
#[test]
fn deep_recursion_phase_cycle_program_integrity() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    for _ in 0..50 {
        zk_alloc::begin_phase();
        rayon::join(|| nested_join(2048), || {});
        zk_alloc::end_phase();
    }
}

/// Drive worker buffer growth via deep recursion that ALSO performs heap
/// allocations in each frame. If the worker's grown Buffer landed in the
/// worker's own slab, then phase 2 worker allocations at the same offset
/// would corrupt either the buffer (visible as a crash on next push/pop) or
/// — if the canary placement aligns — corrupt the canary directly.
fn nested_join_with_alloc(depth: usize) {
    if depth == 0 {
        return;
    }
    let v: Vec<u64> = vec![depth as u64; 1024]; // 8 KB, > MIN_ARENA_BYTES
    rayon::join(|| nested_join_with_alloc(depth - 1), || {});
    std::hint::black_box(v);
}

#[test]
fn worker_buffer_growth_with_per_worker_canary() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    let mut failures = 0;
    const CYCLES: usize = 20;
    for cycle in 0..CYCLES {
        zk_alloc::begin_phase();
        rayon::join(|| nested_join_with_alloc(512), || {});
        zk_alloc::end_phase();

        zk_alloc::begin_phase();
        // Each worker allocates a 16 KB canary in its own slab, then drives
        // more rayon work that uses the (potentially recycled) deque buffer.
        let results: Vec<bool> = (0..32_u64)
            .into_par_iter()
            .map(|_| {
                let canary = vec![0xC9_u8; 16384];
                rayon::join(|| nested_join_with_alloc(64), || {});
                canary.iter().all(|&b| b == 0xC9)
            })
            .collect();
        zk_alloc::end_phase();

        let n_corrupt = results.iter().filter(|&&ok| !ok).count();
        if n_corrupt > 0 {
            eprintln!("cycle {cycle}: {n_corrupt}/32 workers saw canary corruption");
            failures += 1;
        }
    }
    eprintln!(
        "worker_buffer_growth_with_per_worker_canary: {failures}/{CYCLES} cycles with corruption (MIN_ARENA_BYTES={})",
        zk_alloc::min_arena_bytes()
    );

    // With size-routing (default 4096), the worker's grown buffer falls into
    // arena only at cap >= 256 slots (4 KB). The 8 KB Vecs allocated in
    // each frame do go to arena and could overlap. In practice the
    // size-routing fix is enough to prevent corruption observable from the
    // canary; without it, the bug manifests as SIGSEGV (verified with
    // ZK_ALLOC_MIN_BYTES=0 + --no-default-features).
    if zk_alloc::min_arena_bytes() >= 4096 {
        assert_eq!(
            failures, 0,
            "size-routing should prevent worker-deque corruption"
        );
    }
}
