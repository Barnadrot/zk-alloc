//! Characterizes how reliably the rayon/zk-alloc bug fires under variants
//! of Tom's MRE: cold rayon pool, repeated cycles, large canaries, sleep
//! between phases. The goal is to map the "trigger surface" of this bug
//! class: which allocation patterns survive a phase boundary into the
//! next phase's recycled slab, and what the typical corruption profile
//! looks like.

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

fn check_canary(canary: &[u8], expect: u8) -> Option<usize> {
    canary.iter().position(|&b| b != expect)
}

/// Cold rayon: no pre-warm. The very first parallel call happens INSIDE an
/// arena phase. Rayon's thread pool, registry, AND injector blocks all get
/// allocated in the arena slab — much bigger blast radius than the warm
/// case. Not only injector blocks: thread stacks, registry state, sleep
/// pools.
#[test]
#[ignore] // Run manually: cargo test --release --test test_rayon_audit -- --ignored --test-threads=1
fn cold_rayon_inside_arena() {
    zk_alloc::begin_phase();
    // First parallel call ever — allocates the entire rayon pool in arena.
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let canary = vec![0xCD_u8; 64 * 1024];
    rayon::join(|| {}, || {});
    zk_alloc::end_phase();

    let pos = check_canary(&canary, 0xCD);
    assert!(
        pos.is_none(),
        "cold-rayon canary corrupted at offset {}",
        pos.unwrap()
    );
}

/// Repeats Tom's MRE 10 times. If the bug is rare/non-deterministic the
/// average failure offset and frequency tell us how many slots an Injector
/// block has when the slab is recycled.
#[test]
fn repeated_phase_cycles() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    let mut failures = 0;
    for cycle in 0..10 {
        zk_alloc::begin_phase();
        for _ in 0..200 {
            rayon::join(|| {}, || {});
        }
        zk_alloc::end_phase();

        zk_alloc::begin_phase();
        let canary = vec![0xAB_u8; 8192];
        rayon::join(|| {}, || {});
        zk_alloc::end_phase();

        if let Some(pos) = check_canary(&canary, 0xAB) {
            eprintln!("cycle {cycle}: canary corrupted at offset {pos}");
            failures += 1;
        }
    }
    eprintln!("repeated_phase_cycles: {failures}/10 cycles corrupted");
    let fix_active = zk_alloc::min_arena_bytes() >= 256;
    if fix_active {
        assert_eq!(failures, 0, "fix should prevent corruption in all cycles");
    } else {
        assert!(
            failures > 0,
            "expected at least one cycle to corrupt — bug should be reproducible"
        );
    }
}

/// Canary larger than a typical injector block — does the corruption have
/// a bounded blast radius (one block-sized region) or does it cascade?
#[test]
fn large_canary_blast_radius() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    zk_alloc::begin_phase();
    for _ in 0..200 {
        rayon::join(|| {}, || {});
    }
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let canary = vec![0x55_u8; 1 << 20]; // 1 MB
    rayon::join(|| {}, || {});
    zk_alloc::end_phase();

    let mut corruption_runs = Vec::new();
    let mut i = 0;
    while i < canary.len() {
        if canary[i] != 0x55 {
            let start = i;
            while i < canary.len() && canary[i] != 0x55 {
                i += 1;
            }
            corruption_runs.push((start, i - start));
        }
        i += 1;
    }
    if !corruption_runs.is_empty() {
        eprintln!("large_canary corruption runs: {:?}", corruption_runs);
    }
    let fix_active = zk_alloc::min_arena_bytes() >= 256;
    if fix_active {
        assert!(
            corruption_runs.is_empty(),
            "{} corruption runs in 1MB canary (fix should prevent)",
            corruption_runs.len()
        );
    } else {
        assert_eq!(
            corruption_runs.len(),
            1,
            "without fix, expected exactly one block-sized corruption run, got {}",
            corruption_runs.len()
        );
        let (_start, len) = corruption_runs[0];
        assert!(
            len <= 32,
            "expected single JobRef-sized run (<=32B), got {}B",
            len
        );
    }
}

/// Drives rayon::join from a SPAWNED thread, not the main thread. Both go
/// through the injector (only rayon worker threads bypass it via per-worker
/// deque). Confirms the bug is about non-worker callers, not specifically
/// the main thread.
#[test]
fn injector_bug_from_spawned_thread() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    zk_alloc::begin_phase();
    let h = std::thread::spawn(|| {
        for _ in 0..200 {
            rayon::join(|| {}, || {});
        }
    });
    h.join().unwrap();
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    let canary = vec![0xEE_u8; 8192];
    rayon::join(|| {}, || {});
    zk_alloc::end_phase();

    let pos = check_canary(&canary, 0xEE);
    assert!(
        pos.is_none(),
        "spawned-thread canary corrupted at offset {}",
        pos.unwrap()
    );
}
