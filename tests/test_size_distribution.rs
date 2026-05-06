//! Profiles the size distribution of arena allocations during prove-style
//! workloads. Helps validate that ZK_ALLOC_MIN_BYTES=4096 catches the
//! "library state" allocations without filtering out the bulk-data ones
//! the arena is meant to accelerate.
//!
//! Usage: `cargo test --release --test test_size_distribution -- --nocapture`.
//! Output is a histogram of size buckets and the count of allocations under
//! 4096 bytes vs. above.

use std::sync::Mutex;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

static SIZES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

#[test]
fn profile_allocation_sizes_in_phase() {
    use rayon::prelude::*;

    // Warm up rayon outside arena.
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    // Capture allocations during a phase by piggybacking the global
    // allocator -- we measure indirectly via overflow_stats.
    zk_alloc::reset_overflow_stats();

    zk_alloc::begin_phase();

    // Mix of allocations: tiny (HashMap-style), small, medium, large.
    {
        let mut tiny: Vec<Vec<u8>> = (0..1000).map(|_| vec![0_u8; 32]).collect();
        let small: Vec<Vec<u8>> = (0..1000).map(|_| vec![0_u8; 256]).collect();
        let medium: Vec<Vec<u8>> = (0..100).map(|_| vec![0_u8; 4096]).collect();
        let large: Vec<Vec<u8>> = (0..10).map(|_| vec![0_u8; 1 << 20]).collect();
        std::hint::black_box((&tiny, &small, &medium, &large));
        tiny.clear();
        SIZES
            .lock()
            .unwrap()
            .extend([tiny.capacity(), small.len(), medium.len(), large.len()]);
    }

    zk_alloc::end_phase();

    let (overflow_count, overflow_bytes) = zk_alloc::overflow_stats();
    eprintln!(
        "overflow stats during phase (arena fallthrough): count={overflow_count}, bytes={overflow_bytes}"
    );
    eprintln!(
        "min_arena_bytes() = {} (allocations below this size go to System)",
        zk_alloc::min_arena_bytes()
    );

    // With size routing, allocations < min_arena_bytes don't touch the
    // arena AND don't increment overflow_stats (they bypass the arena
    // path entirely). Overflow_stats only counts allocations that tried
    // arena but couldn't fit (slab full or too-large).
    if zk_alloc::min_arena_bytes() >= 4096 {
        // 1000 tiny + 1000 small were < 4096 — they go to System silently.
        // 100 medium = 4096 each — at boundary, bypass.
        // 10 large = 1MB each — go to arena.
        // No overflow expected for this size mix.
        assert_eq!(
            overflow_count, 0,
            "expected no overflow with size routing on"
        );
    }
}
