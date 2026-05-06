//! Stress test for the size-routing fix (ZK_ALLOC_MIN_BYTES). Drives many
//! phase cycles with rayon::join from main thread + canaries, to validate
//! that the fix holds at scale (not just the 3-iter Plonky3 example).
//!
//! Run with `ZK_ALLOC_MIN_BYTES=4096 cargo test --release --test
//! test_size_routing_stress -- --nocapture`. Without the env var the test
//! is expected to fail (bug reproduces).

use rayon::prelude::*;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn many_phase_cycles_with_canaries() {
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    const CYCLES: usize = 100;
    let mut failures = 0;
    for cycle in 0..CYCLES {
        zk_alloc::begin_phase();
        for _ in 0..200 {
            rayon::join(|| {}, || {});
        }
        zk_alloc::end_phase();

        zk_alloc::begin_phase();
        let canary = vec![0xC1_u8; 65536];
        rayon::join(|| {}, || {});
        zk_alloc::end_phase();

        if let Some(pos) = canary.iter().position(|&b| b != 0xC1) {
            eprintln!("cycle {cycle}: canary corrupted at offset {pos}");
            failures += 1;
        }
    }
    eprintln!("many_phase_cycles_with_canaries: {failures}/{CYCLES} corrupted");

    // Default MIN_ARENA_BYTES is 4096 (size-routing fix on by default).
    // With ZK_ALLOC_MIN_BYTES=0, fix is disabled and bug should reproduce.
    let min_bytes_active = zk_alloc::min_arena_bytes() >= 256;
    if min_bytes_active {
        assert_eq!(failures, 0, "fix should prevent ALL corruption");
    } else {
        assert!(failures > 0, "without fix, bug should reproduce");
    }
}
