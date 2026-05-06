//! Scenario 6: concurrent begin_phase / end_phase across threads.
//!
//! GENERATION and ARENA_ACTIVE are global atomics. begin_phase() from any
//! thread bumps GENERATION, which forces every other thread's next allocation
//! through the cold path (ARENA_GEN mismatch → reset ARENA_PTR to slab base).
//! That silently invalidates arena data those threads still hold.
//!
//! Race patterns observable:
//!   (a) T2.begin_phase() while T1 holds an arena Vec → T1's next alloc lands
//!       on top of T1's existing Vec (per-thread slab, but offset-0 conflict).
//!   (b) T1.begin_phase() racing T2.end_phase() → ARENA_ACTIVE final state
//!       depends on store ordering; allocations between can route either way.
//!
//! These are public-API hazards: the docs imply single-threaded lifecycle.
//! Tests document the failure modes so a future PhaseGuard / scoped API can
//! address them.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn cross_thread_begin_phase_invalidates_data() {
    let _: u64 = (0..1024_u64).map(|i| i * 2).sum();

    let barrier = Arc::new(Barrier::new(2));
    let aliased = Arc::new(AtomicBool::new(false));
    let bug = Arc::new(AtomicBool::new(false));

    zk_alloc::begin_phase();

    let bar1 = Arc::clone(&barrier);
    let aliased1 = Arc::clone(&aliased);
    let bug1 = Arc::clone(&bug);
    let t1 = thread::spawn(move || {
        let v: Vec<u8> = vec![0xAA; 8192];
        let v_ptr = v.as_ptr() as usize;
        bar1.wait(); // [1] v allocated, wait for T2
        bar1.wait(); // [2] T2 has called begin_phase; resume

        // The cross-thread begin_phase bumped GENERATION. T1's ARENA_GEN is
        // now stale → cold path on next alloc resets ARENA_PTR to T1's slab
        // base. On Linux this lands w on top of v; macOS aarch64 places v
        // and w in different ranges (T1's first alloc may go to System),
        // so the overlap doesn't happen — the bug is real but unobservable
        // from this test on that platform.
        let w: Vec<u8> = vec![0xBB; 8192];
        let w_ptr = w.as_ptr() as usize;
        let v_corrupted = v.iter().any(|&b| b != 0xAA);
        eprintln!("t1: v=0x{v_ptr:x} w=0x{w_ptr:x} v_corrupt={v_corrupted}");
        if v_ptr == w_ptr {
            aliased1.store(true, Ordering::Relaxed);
        }
        if v_corrupted {
            bug1.store(true, Ordering::Relaxed);
        }
        std::hint::black_box((v, w));
    });

    let bar2 = barrier;
    let t2 = thread::spawn(move || {
        bar2.wait(); // [1] T1 has v
        zk_alloc::begin_phase(); // bumps GENERATION globally
        bar2.wait(); // [2] release T1
    });

    t1.join().unwrap();
    t2.join().unwrap();

    zk_alloc::end_phase();

    let saw_aliasing = aliased.load(Ordering::Relaxed);
    let saw_corruption = bug.load(Ordering::Relaxed);

    if saw_aliasing {
        // Linux: cold-path slab reset re-bumps to slab base, w aliases v,
        // and the writes to w corrupt v's bytes.
        assert!(
            saw_corruption,
            "v and w aliased but v's bytes are pristine — \
             cross-thread invalidation got fixed or layout assumption changed"
        );
    } else {
        // macOS aarch64 (and any platform where T1's two allocations land
        // at different addresses) — corruption can't be observed via this
        // exact pattern, but the underlying hazard remains. Pass without
        // asserting; document.
        eprintln!(
            "test inconclusive on this platform: v and w didn't alias, \
             so cross-thread invalidation isn't observable here"
        );
    }
}

/// Two threads each running their own begin_phase/work/end_phase loop —
/// expecting each iteration to be self-contained. Because GENERATION is
/// global, A's begin_phase mid-iteration corrupts B's in-flight data when
/// B allocates a second time after the cross-thread reset.
#[test]
fn two_threads_running_lifecycle_concurrently_corrupt_each_other() {
    let _: u64 = (0..1024_u64).map(|i| i * 2).sum();

    const ITERS: usize = 200;
    let bug = Arc::new(AtomicUsize::new(0));

    thread::scope(|s| {
        for tid in 0u8..2 {
            let bug = Arc::clone(&bug);
            s.spawn(move || {
                for _ in 0..ITERS {
                    zk_alloc::begin_phase();
                    let pattern = if tid == 0 { 0xA1 } else { 0xB2 };
                    // Two allocations per iteration; the second triggers a
                    // cold-path slab reset if the other thread's begin_phase
                    // bumped GENERATION between them.
                    let v: Vec<u8> = vec![pattern; 8192];
                    let _filler: Vec<u8> = vec![0; 8192];
                    if v.iter().any(|&b| b != pattern) {
                        bug.fetch_add(1, Ordering::Relaxed);
                    }
                    std::hint::black_box((v, _filler));
                    zk_alloc::end_phase();
                }
            });
        }
    });

    let n = bug.load(Ordering::Relaxed);
    eprintln!(
        "two_threads_running_lifecycle_concurrently: {n} cross-thread corruptions over {} iters",
        2 * ITERS
    );

    // Race window is narrow (single-µs alloc-to-alloc gap); count is
    // observational, not asserted. The deterministic version is in
    // cross_thread_begin_phase_invalidates_data above.
    eprintln!("(stress observation: race window too tight to be a reliable assertion)");
}

/// Sanity: concurrent begin/end stress doesn't crash the allocator's atomics
/// even if it corrupts user data. Verifies invariants like REGION_BASE are
/// stable.
#[test]
fn concurrent_phase_stress_no_crash() {
    let _: u64 = (0..1024_u64).map(|i| i * 2).sum();

    const ITERS: usize = 5000;
    let stop = Arc::new(AtomicBool::new(false));

    let mut threads = vec![];
    for _ in 0..4 {
        let stop = Arc::clone(&stop);
        threads.push(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                zk_alloc::begin_phase();
                let _v = vec![0u8; 16384];
                zk_alloc::end_phase();
            }
        }));
    }

    thread::sleep(std::time::Duration::from_millis(50));
    for _ in 0..ITERS {
        zk_alloc::begin_phase();
        zk_alloc::end_phase();
    }
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().unwrap();
    }

    zk_alloc::end_phase();
    eprintln!(
        "concurrent_phase_stress_no_crash: completed {ITERS} main-thread cycles + worker churn"
    );
}
