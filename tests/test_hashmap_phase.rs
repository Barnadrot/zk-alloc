//! Bonus: HashMap retained across a phase boundary.
//!
//! HashMap (hashbrown under the hood) allocates one contiguous block for its
//! ctrl bytes + bucket array. When held across a phase boundary, the next
//! phase's first large allocation overwrites the entire structure. The
//! corrupted ctrl bytes (typically all 0x55 from filler) make every probe
//! position appear "full" — but no entry actually matches the hash, so
//! HashMap.get enters an infinite probe loop.
//!
//! Detection: spawn a worker thread doing the get(), use a timeout. A hang
//! is the corruption signal.
//!
//! With with_capacity(2048) HashMap<u64, u64>, the bucket allocation is
//! ~32 KB — above MIN_ARENA_BYTES (default 4096) so it lands in arena.

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[global_allocator]
static A: zk_alloc::ZkAllocator = zk_alloc::ZkAllocator;

#[test]
fn hashmap_corrupted_across_phase_boundary() {
    zk_alloc::begin_phase();
    let mut m: HashMap<u64, u64> = HashMap::with_capacity(4096);
    for i in 0..2000_u64 {
        m.insert(i, i.wrapping_mul(i).wrapping_add(0xDEADBEEF));
    }
    let pre_check = m.get(&100).copied();
    assert_eq!(
        pre_check,
        Some(100_u64.wrapping_mul(100).wrapping_add(0xDEADBEEF)),
        "phase-1 invariant broken before boundary"
    );
    zk_alloc::end_phase();

    zk_alloc::begin_phase();
    // 1 MB filler at slab+0 — overwrites HashMap's bucket region.
    let filler: Vec<u8> = vec![0x55; 1 << 20];
    let filler_lo = filler.as_ptr() as usize;
    let filler_hi = filler_lo + filler.len();
    eprintln!("filler=[0x{filler_lo:x}, 0x{filler_hi:x})");

    // Move m into a worker thread; detect hang via channel timeout.
    let (tx, rx) = mpsc::channel();
    let _hang_thread = thread::spawn(move || {
        let i = 100_u64;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.get(&i).copied()));
        let _ = tx.send(result);
        // Don't drop m — its state is corrupted, drop chain would also be UB.
        std::mem::forget(m);
    });

    let outcome = rx.recv_timeout(Duration::from_secs(2));
    match &outcome {
        Ok(Ok(Some(v))) => eprintln!("HashMap.get returned: {v}"),
        Ok(Ok(None)) => eprintln!("HashMap.get returned None (entry vanished)"),
        Ok(Err(_)) => eprintln!("HashMap.get panicked"),
        Err(_) => eprintln!("HashMap.get TIMED OUT (infinite probe on corrupted ctrl bytes)"),
    }
    drop(filler);
    zk_alloc::end_phase();

    let expected = 100_u64.wrapping_mul(100).wrapping_add(0xDEADBEEF);
    let pristine = matches!(outcome, Ok(Ok(Some(v))) if v == expected);
    assert!(
        !pristine,
        "expected HashMap corruption (timeout / panic / wrong value), got pristine"
    );
    eprintln!(
        "BUG CONFIRMED: HashMap retained across phase boundary corrupted (outcome: {outcome:?})"
    );
}
