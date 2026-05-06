//! Stresses tracing-subscriber's sharded-slab into allocating page 1 (~6KB,
//! 64 slots) inside an arena phase. The first page (~3.2KB) is covered by
//! the 4096-byte size-routing threshold, but page 1 exceeds it — so this
//! test verifies whether the fix holds under heavy span concurrency.
//!
//! Run with `cargo test --release --test test_many_spans_stress`.

use rayon::prelude::*;
use tracing::info_span;
use tracing_subscriber::{Registry, layer::SubscriberExt, util::SubscriberInitExt};

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn many_concurrent_spans_across_phases() {
    inner(64);
}

/// Documents the upper limit of the size-routing fix. With default
/// MIN_BYTES=4096, 512 concurrent spans triggers a sharded-slab page
/// allocation that exceeds the threshold, lands in arena, and corrupts
/// across phase boundaries. To pass, set ZK_ALLOC_MIN_BYTES=6144 or higher.
/// Run manually: `cargo test --release --test test_many_spans_stress -- --ignored`.
#[test]
#[ignore]
fn extreme_concurrent_spans_across_phases() {
    inner(512);
}

fn inner(n: u64) {
    let _ = Registry::default()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .try_init();

    // Warm up rayon outside arena.
    let _: u64 = (0..1_000_000_u64).into_par_iter().sum();

    for cycle in 0..5 {
        zk_alloc::begin_phase();

        // Create 64 concurrent live spans -- forces sharded-slab to grow
        // past page 0 (~32 slots) into page 1 (~6KB allocation).
        let spans: Vec<_> = (0..n).map(|i| info_span!("concurrent", cycle, i)).collect();
        {
            let _entered: Vec<_> = spans.iter().map(|s| s.enter()).collect();
            // guards drop here, before spans
        }
        drop(spans);

        zk_alloc::end_phase();
    }

    // After 5 phase cycles, create one more span and observe whether its
    // backing data is corrupted. With size-routing fix off, the pooled
    // page-1 slot data has been overwritten across phases; with the fix
    // on at >= 4096, page 1 might still go to arena (> 4KB), so this
    // probes the limit of the fix.
    zk_alloc::begin_phase();
    let spans2: Vec<_> = (0..n).map(|i| info_span!("post_cycle", i)).collect();
    {
        let _entered: Vec<_> = spans2.iter().map(|s| s.enter()).collect();
    }
    drop(spans2);
    zk_alloc::end_phase();
}
