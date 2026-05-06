//! Scenario 4: thread pool resizing / building mid-phase.
//!
//! ThreadPoolBuilder::build() allocates a Registry, per-worker ThreadInfo
//! arrays, initial Worker deques, and a Sleep struct. If built during an
//! active phase, these allocations land in the arena. The pool is held by
//! the user across phase boundaries; Registry pointers reference arena
//! memory that gets recycled on the next begin_phase, so subsequent
//! .install() calls walk corrupted scheduler state.
//!
//! Most of these allocations are sub-KB (per F6/F13 audit) and bypass arena
//! under default size-routing. Empirical test: build a fresh ThreadPool
//! mid-phase, cross a boundary, install work, and look for crashes / hangs.

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn build_threadpool_during_phase_then_use_across_boundary() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    const CYCLES: usize = 10;
    for cycle in 0..CYCLES {
        zk_alloc::begin_phase();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .thread_name(move |i| format!("test-pool-{cycle}-{i}"))
            .build()
            .expect("build pool");
        zk_alloc::end_phase();

        // Phase boundary. Pool's Registry, ThreadInfo arrays, deques: any
        // that were arena-allocated are now in recycled territory.
        zk_alloc::begin_phase();
        // Force the pool to use its Registry via .install + par work. If
        // any pointer-walking state was in arena and got recycled, this
        // crashes or hangs.
        let result: u64 = pool.install(|| (0..1024_u64).into_par_iter().sum());
        assert_eq!(result, 1024 * 1023 / 2);
        zk_alloc::end_phase();

        drop(pool);
    }
    eprintln!(
        "build_threadpool_during_phase: {CYCLES} cycles OK (MIN_ARENA_BYTES={})",
        zk_alloc::min_arena_bytes()
    );
}

#[test]
fn many_threadpool_builds_during_phase() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    for cycle in 0..20 {
        zk_alloc::begin_phase();
        // Build, use immediately within phase, drop. All within one phase.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(move |i| format!("ephemeral-{cycle}-{i}"))
            .build()
            .expect("build pool");
        let result: u64 = pool.install(|| (0..512_u64).into_par_iter().sum());
        assert_eq!(result, 512 * 511 / 2);
        drop(pool);
        zk_alloc::end_phase();
    }
}

/// Build pool BEFORE any phase, use it across many phases. Pool's allocations
/// are pre-phase (System); should be fully isolated from phase resets.
#[test]
fn pre_phase_threadpool_used_across_many_phases() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .thread_name(|i| format!("pre-phase-{i}"))
        .build()
        .expect("build pool");

    for _ in 0..50 {
        zk_alloc::begin_phase();
        let result: u64 = pool.install(|| (0..1024_u64).into_par_iter().sum());
        assert_eq!(result, 1024 * 1023 / 2);
        zk_alloc::end_phase();
    }
    drop(pool);
}
